use crate::error::{Error, Result, io};
use crate::progress::{Progress, Sink};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE};
use sha1::{Digest, Sha1};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct FileSpec {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub verify_existing: bool,
}

pub struct Http {
    client: reqwest::blocking::Client,
    mirror: String,
}

const SEGMENT_THRESHOLD: u64 = 8 * 1024 * 1024;
const SEGMENT_COUNT: u64 = 4;

#[derive(Debug)]
pub struct RangeResponse {
    pub bytes: Vec<u8>,
    pub total: Option<u64>,
}

impl Http {
    pub fn new(mirror: &str) -> Result<Self> {
        // reqwest 0.13 使用 rustls-no-provider；显式安装 ring provider，
        // 避免 Windows 构建时引入 aws-lc-sys 的 C/汇编工具链。
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("nailong/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(180))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| Error::Net {
                url: "client".into(),
                source: e,
            })?;
        Ok(Self {
            client,
            mirror: mirror.to_string(),
        })
    }

    pub fn url(&self, url: &str) -> String {
        rewrite_mirror(url, &self.mirror)
    }

    /// Primary URL plus a cross-source fallback.  Mirrors are useful most of
    /// the time but occasionally 404/rate-limit; trying the other side costs
    /// one request only after a real failure.
    fn candidate_urls(&self, original: &str) -> Vec<String> {
        let primary = self.url(original);
        let mut out = vec![primary.clone()];
        if primary != original {
            out.push(original.to_string());
        } else {
            let mirror = rewrite_mirror(original, "bmcl");
            if mirror != original {
                out.push(mirror);
            }
        }
        if out.len() > 1 && out[0] == out[1] {
            out.pop();
        }
        out
    }

    pub fn get_text(&self, url: &str) -> Result<String> {
        let url = self.url(url);
        let mut last = None;
        for attempt in 0..3 {
            match self.client.get(&url).send() {
                Ok(resp) => {
                    let status = resp.status();
                    if !status.is_success() {
                        last = Some(Error::Http {
                            url: url.clone(),
                            status: status.as_u16(),
                        });
                    } else {
                        return resp.text().map_err(|e| Error::Net {
                            url: url.clone(),
                            source: e,
                        });
                    }
                }
                Err(e) => {
                    last = Some(Error::Net {
                        url: url.clone(),
                        source: e,
                    })
                }
            }
            std::thread::sleep(Duration::from_millis(400 * (attempt + 1) as u64));
        }
        Err(last.unwrap_or_else(|| Error::Install(format!("请求失败: {url}"))))
    }

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let text = self.get_text(url)?;
        serde_json::from_str(&text).map_err(|e| Error::Install(format!("JSON 解析失败: {e}")))
    }

    /// Fetch one inclusive byte range.  `Ok(None)` means the server ignored the
    /// Range header and started a normal 200 response; callers must fall back
    /// to a full download instead of treating that body as a range.
    pub fn get_range(&self, url: &str, start: u64, end: u64) -> Result<Option<RangeResponse>> {
        if end < start {
            return Err(Error::Install(format!("无效的 Range: {start}-{end}")));
        }
        let url = self.url(url);
        let mut last = None;
        for attempt in 0..3u32 {
            match self.try_get_range(&url, start, end) {
                Ok(Some(response)) => return Ok(Some(response)),
                Ok(None) => return Ok(None),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(400 * (attempt + 1) as u64));
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::Install(format!("请求失败: {url}"))))
    }

    pub fn range_len(&self, url: &str) -> Result<Option<u64>> {
        Ok(self
            .get_range(url, 0, 0)?
            .and_then(|response| response.total))
    }

    fn try_get_range(&self, url: &str, start: u64, end: u64) -> Result<Option<RangeResponse>> {
        let response = self
            .client
            .get(url)
            .header(RANGE, format!("bytes={start}-{end}"))
            // Byte ranges only make sense on the raw representation.
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|e| Error::Net {
                url: url.into(),
                source: e,
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let total = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range_total);
            let bytes = response
                .bytes()
                .map_err(|e| Error::Net {
                    url: url.into(),
                    source: e,
                })?
                .to_vec();
            let expected = end.saturating_sub(start).saturating_add(1);
            if bytes.len() as u64 != expected {
                return Err(Error::Install(format!(
                    "Range 响应长度不对: 期望 {expected}，实际 {} ({url})",
                    bytes.len()
                )));
            }
            Ok(Some(RangeResponse { bytes, total }))
        } else if status == reqwest::StatusCode::OK {
            // Server sent the whole file.  Do not read it: the caller will
            // choose a normal download path with progress and checksums.
            Ok(None)
        } else {
            Err(Error::Http {
                url: url.into(),
                status: status.as_u16(),
            })
        }
    }

    fn try_download_segmented(
        &self,
        url: &str,
        dest: &Path,
        total: u64,
        expected: &str,
        batch: Option<&Batch>,
    ) -> Result<()> {
        let chunk = total.div_ceil(SEGMENT_COUNT);
        let results: Vec<Result<()>> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for index in 0..SEGMENT_COUNT {
                let start = index * chunk;
                if start >= total {
                    break;
                }
                let end = (start + chunk - 1).min(total - 1);
                let part = segment_path(dest, index);
                let client = &self.client;
                handles.push(
                    scope.spawn(move || download_segment(client, url, &part, start, end, batch)),
                );
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(Error::Install("分段下载线程异常退出".into())))
                })
                .collect()
        });
        for result in results {
            result?;
        }

        let merged = dest.with_extension(format!("merged-{}", std::process::id()));
        let mut out = File::create(&merged).map_err(|e| io(&merged, e))?;
        for index in 0..SEGMENT_COUNT {
            let part = segment_path(dest, index);
            if !part.exists() {
                continue;
            }
            let mut input = File::open(&part).map_err(|e| io(&part, e))?;
            std::io::copy(&mut input, &mut out).map_err(|e| io(&part, e))?;
        }
        out.sync_all().map_err(|e| io(&merged, e))?;
        let actual_size = merged.metadata().map(|m| m.len()).unwrap_or(0);
        if actual_size != total {
            let _ = std::fs::remove_file(&merged);
            cleanup_parts(dest);
            return Err(Error::Install(format!(
                "分段合并后大小不符: 期望 {total}, 实际 {actual_size}"
            )));
        }
        let actual = sha1_file(&merged)?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(&merged);
            cleanup_parts(dest);
            return Err(Error::Checksum {
                path: dest.to_path_buf(),
                expected: expected.into(),
                actual,
            });
        }
        std::fs::rename(&merged, dest).map_err(|e| {
            let _ = std::fs::remove_file(&merged);
            io(dest, e)
        })?;
        cleanup_parts(dest);
        Ok(())
    }

    pub fn download(&self, spec: &FileSpec) -> Result<()> {
        self.download_inner(spec, None)
    }

    pub fn download_with(&self, spec: FileSpec, label: &str, sink: &Sink) -> Result<()> {
        self.download_many_with(vec![spec], label, sink).map(|_| ())
    }

    fn download_inner(&self, spec: &FileSpec, batch: Option<&Batch>) -> Result<()> {
        if spec.dest.exists() {
            let actual_size = spec.dest.metadata().map(|m| m.len()).unwrap_or(0);
            let size_ok = spec.size.is_none_or(|s| actual_size == s);
            if size_ok && !spec.verify_existing {
                if let Some(batch) = batch {
                    batch.file_done(actual_size);
                }
                return Ok(());
            }
            if size_ok {
                if let Some(expected) = &spec.sha1 {
                    let actual = sha1_file(&spec.dest)?;
                    if actual.eq_ignore_ascii_case(expected) {
                        if let Some(batch) = batch {
                            batch.file_done(actual_size);
                        }
                        return Ok(());
                    }
                    let _ = std::fs::remove_file(&spec.dest);
                } else {
                    if let Some(batch) = batch {
                        batch.file_done(actual_size);
                    }
                    return Ok(());
                }
            }
        }
        if let Some(parent) = spec.dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        // A single large file (the 26 MB client jar is the common case) gets
        // four parallel ranges.  Batches of many small files stay one request
        // per file to avoid a 16 x 4 connection explosion.
        let single_file = batch.is_none_or(|b| b.files_total <= 2);
        if single_file
            && let Some(size) = spec.size
            && size >= SEGMENT_THRESHOLD
            && let Some(expected) = spec.sha1.as_deref()
        {
            let primary = self.url(&spec.url);
            let before = batch.map(|b| b.bytes_done.load(Ordering::Relaxed));
            let range_ok = matches!(
                self.try_get_range(&primary, 0, 0),
                Ok(Some(response)) if response.total == Some(size)
            );
            if range_ok {
                match self.try_download_segmented(&primary, &spec.dest, size, expected, batch) {
                    Ok(()) => {
                        if let Some(batch) = batch {
                            batch.file_done(0);
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        if let (Some(batch), Some(before)) = (batch, before) {
                            let after = batch.bytes_done.load(Ordering::Relaxed);
                            batch
                                .bytes_done
                                .fetch_sub(after.saturating_sub(before), Ordering::Relaxed);
                        }
                        cleanup_parts(&spec.dest);
                        println!("! 分段下载失败，回退单连接: {e}");
                    }
                }
            }
        }

        let urls = self.candidate_urls(&spec.url);
        let mut last = None;
        for attempt in 0..3u32 {
            let before = batch.map(|b| b.bytes_done.load(Ordering::Relaxed));
            let mut attempt_error = None;
            for (index, url) in urls.iter().enumerate() {
                if index > 0 {
                    println!("! 切换备用源: {url}");
                }
                match self.try_download(url, &spec.dest, spec.sha1.as_deref(), batch) {
                    Ok(()) => {
                        if let Some(batch) = batch {
                            batch.file_done(0);
                        }
                        return Ok(());
                    }
                    Err(e) => attempt_error = Some(e),
                }
            }
            if let (Some(batch), Some(before)) = (batch, before) {
                let after = batch.bytes_done.load(Ordering::Relaxed);
                batch
                    .bytes_done
                    .fetch_sub(after.saturating_sub(before), Ordering::Relaxed);
            }
            last = attempt_error;
            std::thread::sleep(Duration::from_millis(500 * (attempt + 1) as u64));
        }
        if let Some(batch) = batch {
            batch.file_done(0);
        }
        Err(last.unwrap_or_else(|| {
            Error::Install(format!(
                "下载失败: {}",
                urls.first().map(String::as_str).unwrap_or("")
            ))
        }))
    }

    fn try_download(
        &self,
        url: &str,
        dest: &Path,
        expected: Option<&str>,
        batch: Option<&Batch>,
    ) -> Result<()> {
        let resp = self.client.get(url).send().map_err(|e| Error::Net {
            url: url.into(),
            source: e,
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(Error::Http {
                url: url.into(),
                status: status.as_u16(),
            });
        }
        let tmp = dest.with_extension(format!("part-{}", std::process::id()));
        let mut file = File::create(&tmp).map_err(|e| io(&tmp, e))?;
        let mut hasher = Sha1::new();
        let mut reader = resp;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| io(dest, e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| io(&tmp, e))?;
            if let Some(batch) = batch {
                batch.bytes_done.fetch_add(n as u64, Ordering::Relaxed);
                batch.emit(false);
            }
        }
        file.sync_all().map_err(|e| io(&tmp, e))?;
        if let Some(expected) = expected {
            let actual = hex(&hasher.finalize());
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = std::fs::remove_file(&tmp);
                return Err(Error::Checksum {
                    path: dest.to_path_buf(),
                    expected: expected.into(),
                    actual,
                });
            }
        }
        std::fs::rename(&tmp, dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(dest, e)
        })?;
        Ok(())
    }

    pub fn download_many(&self, specs: Vec<FileSpec>, label: &str) -> Result<usize> {
        let bar = Arc::new(ProgressBar::new(0));
        bar.set_style(
            ProgressStyle::with_template("{spinner:.green} {msg} {pos}/{len} [{bar:30}]").unwrap(),
        );
        bar.set_message(label.to_string());
        let sink: Sink = {
            let bar = bar.clone();
            Arc::new(move |p: Progress| {
                let use_bytes = p.bytes_total > 0;
                bar.set_length(if use_bytes {
                    p.bytes_total
                } else {
                    p.files_total
                });
                bar.set_position(if use_bytes {
                    p.bytes_done
                } else {
                    p.files_done
                });
                bar.set_message(p.stage);
            })
        };
        let result = self.download_many_with(specs, label, &sink);
        bar.finish_and_clear();
        result
    }

    pub fn download_many_with(
        &self,
        specs: Vec<FileSpec>,
        label: &str,
        sink: &Sink,
    ) -> Result<usize> {
        if specs.is_empty() {
            return Ok(0);
        }
        let total_bytes = if specs.iter().all(|s| s.size.is_some()) {
            specs.iter().filter_map(|s| s.size).sum()
        } else {
            0
        };
        let batch = Batch {
            sink,
            stage: label.to_string(),
            files_done: AtomicU64::new(0),
            files_total: specs.len() as u64,
            bytes_done: AtomicU64::new(0),
            bytes_total: total_bytes,
            last_emit_ms: AtomicU64::new(0),
        };
        batch.emit(true);
        let workers = specs.len().clamp(1, 16);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .map_err(|e| Error::Install(e.to_string()))?;
        let errors: Vec<String> = pool.install(|| {
            specs
                .par_iter()
                .filter_map(|s| match self.download_inner(s, Some(&batch)) {
                    Ok(()) => None,
                    Err(e) => Some(format!("{}: {e}", s.dest.display())),
                })
                .collect()
        });
        batch.emit(true);
        if errors.is_empty() {
            Ok(specs.len())
        } else {
            let head = errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            Err(Error::Install(format!(
                "{} 个文件下载失败:\n{}",
                errors.len(),
                head
            )))
        }
    }
}

fn segment_path(dest: &Path, index: u64) -> PathBuf {
    dest.with_extension(format!("dlpart-{index}"))
}

fn cleanup_parts(dest: &Path) {
    for index in 0..SEGMENT_COUNT {
        let _ = std::fs::remove_file(segment_path(dest, index));
    }
}

fn download_segment(
    client: &reqwest::blocking::Client,
    url: &str,
    part: &Path,
    start: u64,
    end: u64,
    batch: Option<&Batch>,
) -> Result<()> {
    let want = end.saturating_sub(start).saturating_add(1);
    let mut have = part.metadata().map(|m| m.len()).unwrap_or(0);
    if have > want {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    if have == want {
        return Ok(());
    }
    let from = start + have;
    let response = client
        .get(url)
        .header(RANGE, format!("bytes={from}-{end}"))
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .map_err(|e| Error::Net {
            url: url.into(),
            source: e,
        })?;
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(Error::Install(format!(
            "分段源未返回 206: {}（{url}）",
            response.status()
        )));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(part)
        .map_err(|e| io(part, e))?;
    let mut reader = response;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| io(part, e))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| io(part, e))?;
        if let Some(batch) = batch {
            batch.bytes_done.fetch_add(n as u64, Ordering::Relaxed);
            batch.emit(false);
        }
    }
    file.sync_all().map_err(|e| io(part, e))?;
    let actual = part.metadata().map(|m| m.len()).unwrap_or(0);
    if actual != want {
        return Err(Error::Install(format!(
            "分段长度不符: 期望 {want}, 实际 {actual}（{}）",
            part.display()
        )));
    }
    Ok(())
}

struct Batch<'a> {
    sink: &'a Sink,
    stage: String,
    files_done: AtomicU64,
    files_total: u64,
    bytes_done: AtomicU64,
    bytes_total: u64,
    last_emit_ms: AtomicU64,
}

impl Batch<'_> {
    fn file_done(&self, size: u64) {
        if size > 0 {
            self.bytes_done.fetch_add(size, Ordering::Relaxed);
        }
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.emit(true);
    }

    fn emit(&self, force: bool) {
        let now = now_ms();
        let last = self.last_emit_ms.load(Ordering::Relaxed);
        if !force && now.saturating_sub(last) < 80 {
            return;
        }
        if self
            .last_emit_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let progress = Progress {
            stage: self.stage.clone(),
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total,
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total,
            indeterminate: false,
        };
        (self.sink)(progress);
    }
}

fn parse_content_range_total(value: &str) -> Option<u64> {
    let value = value.strip_prefix("bytes ")?;
    let (_, total) = value.split_once('/')?;
    total.trim().parse().ok()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).map_err(|e| io(path, e))?;
    let mut h = Sha1::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| io(path, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn rewrite_mirror(url: &str, mirror: &str) -> String {
    if mirror != "bmcl" && mirror != "china" {
        return url.to_string();
    }
    let rules = [
        (
            "https://piston-meta.mojang.com",
            "https://bmclapi2.bangbang93.com",
        ),
        (
            "https://launchermeta.mojang.com",
            "https://bmclapi2.bangbang93.com",
        ),
        (
            "https://launcher.mojang.com",
            "https://bmclapi2.bangbang93.com",
        ),
        (
            "https://piston-data.mojang.com",
            "https://bmclapi2.bangbang93.com",
        ),
        (
            "https://libraries.minecraft.net",
            "https://bmclapi2.bangbang93.com/maven",
        ),
        (
            "https://resources.download.minecraft.net",
            "https://bmclapi2.bangbang93.com/assets",
        ),
        (
            "https://maven.fabricmc.net",
            "https://bmclapi2.bangbang93.com/maven",
        ),
        (
            "https://maven.minecraftforge.net",
            "https://bmclapi2.bangbang93.com/maven",
        ),
        (
            "https://maven.neoforged.net/releases",
            "https://bmclapi2.bangbang93.com/maven",
        ),
    ];
    for (from, to) in rules {
        if let Some(rest) = url.strip_prefix(from) {
            return format!("{to}{rest}");
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_candidates_keep_primary_first() {
        let http = Http::new("bmcl").unwrap();
        let original = "https://libraries.minecraft.net/com/example/lib.jar";
        let urls = http.candidate_urls(original);
        assert!(urls[0].contains("bmclapi"));
        assert_eq!(urls[1], original);

        let official = Http::new("official").unwrap();
        let urls = official.candidate_urls(original);
        assert_eq!(urls[0], original);
        assert!(urls[1].contains("bmclapi"));
    }

    #[test]
    fn segment_parts_have_stable_names() {
        let dest = Path::new("/tmp/example/client.jar");
        assert_eq!(
            segment_path(dest, 0),
            Path::new("/tmp/example/client.dlpart-0")
        );
        assert_eq!(
            segment_path(dest, 3),
            Path::new("/tmp/example/client.dlpart-3")
        );
    }
}
