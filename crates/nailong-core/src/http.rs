use crate::cancel;
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
    /// 进度里显示的名字；None 时用文件名。资源文件的文件名是哈希，所以给它真名。
    pub label: Option<String>,
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
            .user_agent(format!(
                "cairn/{} (+https://github.com/dkjsiogu/cairn)",
                env!("CARGO_PKG_VERSION")
            ))
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

    /// 这次请求先连的地址。
    pub fn url(&self, url: &str) -> String {
        self.candidate_urls(url).swap_remove(0)
    }

    /// 一个地址的候选列表，按先后排好。规矩见 [`fallback_urls`]。
    fn candidate_urls(&self, original: &str) -> Vec<String> {
        let mirror_first = self.mirror == "bmcl" || self.mirror == "china";
        let mut out = fallback_urls(original, mirror_first);
        // 官方刚连不上过：几分钟内同一主机直接走后备，不必每个请求都等它超时。
        if out.len() > 1
            && out[0] == original
            && let Some(host) = host_of(original)
            && fallback_for(&host).is_some()
        {
            out.rotate_left(1);
        }
        out
    }

    pub fn get_text(&self, url: &str) -> Result<String> {
        self.get_with(url, &[], |response, used| {
            let text = response.text().map_err(|e| Error::Net {
                url: used.to_string(),
                source: e,
            })?;
            if host_of(used).as_deref() == Some(MCIM_HOST) {
                note_synced(&text);
            }
            Ok(text)
        })
    }

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let text = self.get_text(url)?;
        serde_json::from_str(&text).map_err(|e| Error::Install(format!("JSON 解析失败: {e}")))
    }

    /// 依次试候选地址，每轮都试一遍，最多三轮。
    ///
    /// - 有人明确说「没有」（404 之类）就不再重试：官方说的直接作数；镜像说的，
    ///   等官方也连不上时作数，镜像有时比官方晚，所以先问一下官方；
    /// - 密钥只发给原本的主机，换到镜像时去掉；
    /// - 后备成功时记下来，界面能告诉人现在用的是哪个镜像。
    fn get_with<T>(
        &self,
        original: &str,
        headers: &[(&str, &str)],
        read: impl Fn(reqwest::blocking::Response, &str) -> Result<T>,
    ) -> Result<T> {
        cancel::check()?;
        let urls = self.candidate_urls(original);
        let home = host_of(original);
        let mut last = None;
        let mut official_failed = false;
        // 镜像明确说没有的答案；这个镜像之后不再问。
        let mut answered: Option<(usize, Error)> = None;
        for attempt in 0..3u32 {
            for (index, url) in urls.iter().enumerate() {
                if answered.as_ref().is_some_and(|(i, _)| *i == index) {
                    continue;
                }
                cancel::check()?;
                let official = url == original;
                let mut request = self.client.get(url);
                for (key, value) in headers_for(headers, original, url) {
                    request = request.header(*key, *value);
                }
                let failure = match request.send() {
                    Ok(response) if response.status().is_success() => match read(response, url) {
                        Ok(value) => {
                            if let Some(home) = &home {
                                if official {
                                    clear_fallback(home);
                                } else if official_failed {
                                    note_fallback(home, url);
                                }
                            }
                            return Ok(value);
                        }
                        Err(e @ Error::Cancelled) => return Err(e),
                        Err(e) => e,
                    },
                    Ok(response) => {
                        let status = response.status().as_u16();
                        let error = Error::Http {
                            url: url.clone(),
                            status,
                        };
                        if is_final_status(status) {
                            if official || official_failed {
                                return Err(error);
                            }
                            answered = Some((index, error));
                            continue;
                        }
                        error
                    }
                    Err(e) => Error::Net {
                        url: url.clone(),
                        source: e,
                    },
                };
                if official {
                    official_failed = true;
                    if let Some((_, error)) = answered.take() {
                        return Err(error);
                    }
                }
                last = Some(failure);
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(400 * (attempt + 1) as u64));
            }
        }
        Err(answered
            .map(|(_, error)| error)
            .or(last)
            .unwrap_or_else(|| Error::Install(format!("请求失败: {original}"))))
    }

    /// POST JSON，返回 (status, body)。4xx 也把 body 交回去，方便读 OAuth 错误码。
    pub fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<(u16, serde_json::Value)> {
        cancel::check()?;
        let mut last = None;
        for attempt in 0..3u32 {
            cancel::check()?;
            let mut request = self.client.post(url).json(body);
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
            match request.send() {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let text = response.text().map_err(|e| Error::Net {
                        url: url.to_string(),
                        source: e,
                    })?;
                    let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
                    if status < 500 {
                        return Ok((status, value));
                    }
                    last = Some(Error::Http {
                        url: url.to_string(),
                        status,
                    });
                }
                Err(e) => {
                    last = Some(Error::Net {
                        url: url.to_string(),
                        source: e,
                    })
                }
            }
            std::thread::sleep(Duration::from_millis(300 * (attempt + 1) as u64));
        }
        Err(last.unwrap_or_else(|| Error::Install(format!("请求失败: {url}"))))
    }

    /// POST 表单，返回 (status, body)。
    pub fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, serde_json::Value)> {
        cancel::check()?;
        let mut last = None;
        for attempt in 0..3u32 {
            cancel::check()?;
            match self.client.post(url).form(form).send() {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let text = response.text().map_err(|e| Error::Net {
                        url: url.to_string(),
                        source: e,
                    })?;
                    let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
                    if status < 500 {
                        return Ok((status, value));
                    }
                    last = Some(Error::Http {
                        url: url.to_string(),
                        status,
                    });
                }
                Err(e) => {
                    last = Some(Error::Net {
                        url: url.to_string(),
                        source: e,
                    })
                }
            }
            std::thread::sleep(Duration::from_millis(300 * (attempt + 1) as u64));
        }
        Err(last.unwrap_or_else(|| Error::Install(format!("请求失败: {url}"))))
    }

    /// GET 一次（不换镜像），返回状态码、某个响应头和正文。认证服务器的 ALI 头要用它。
    pub fn get_with_header(
        &self,
        url: &str,
        header: &str,
    ) -> Result<(u16, Option<String>, String)> {
        cancel::check()?;
        let response = self.client.get(url).send().map_err(|e| Error::Net {
            url: url.to_string(),
            source: e,
        })?;
        let status = response.status().as_u16();
        let value = response
            .headers()
            .get(header)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let text = response.text().map_err(|e| Error::Net {
            url: url.to_string(),
            source: e,
        })?;
        Ok((status, value, text))
    }

    /// 取回二进制内容（图标之类的小文件），上限 4 MiB，避免误拉大文件。
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        self.get_with(url, &[], |response, used| {
            let bytes = response.bytes().map_err(|e| Error::Net {
                url: used.to_string(),
                source: e,
            })?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err(Error::Install(format!("文件过大: {used}")));
            }
            Ok(bytes.to_vec())
        })
    }

    pub fn get_json_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        self.get_with(url, headers, |response, used| {
            let text = response.text().map_err(|e| Error::Net {
                url: used.to_string(),
                source: e,
            })?;
            if host_of(used).as_deref() == Some(MCIM_HOST) {
                note_synced(&text);
            }
            serde_json::from_str(&text).map_err(|e| Error::Install(format!("JSON 解析失败: {e}")))
        })
    }

    /// Fetch one inclusive byte range.  `Ok(None)` means the server ignored the
    /// Range header and started a normal 200 response; callers must fall back
    /// to a full download instead of treating that body as a range.
    pub fn get_range(&self, url: &str, start: u64, end: u64) -> Result<Option<RangeResponse>> {
        cancel::check()?;
        if end < start {
            return Err(Error::Install(format!("无效的 Range: {start}-{end}")));
        }
        let url = self.url(url);
        let mut last = None;
        for attempt in 0..3u32 {
            cancel::check()?;
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
        cancel::check()?;
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
        cancel::check()?;
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
        cancel::check()?;

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
        cancel::check()?;
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
        // 真要下载了：告诉界面在下哪个文件。已有的文件不报名字，补全时列表才不会被刷满。
        let label = spec.label.clone().unwrap_or_else(|| {
            spec.dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });
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
                if let Some(batch) = batch {
                    batch.started(&label, &primary);
                }
                match self.try_download_segmented(&primary, &spec.dest, size, expected, batch) {
                    Ok(()) => {
                        if let Some(batch) = batch {
                            batch.file_done(0);
                        }
                        return Ok(());
                    }
                    Err(e @ Error::Cancelled) => return Err(e),
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
        let home = host_of(&spec.url);
        let mut official_failed = false;
        let mut last = None;
        for attempt in 0..3u32 {
            let before = batch.map(|b| b.bytes_done.load(Ordering::Relaxed));
            let mut attempt_error = None;
            for (index, url) in urls.iter().enumerate() {
                if index > 0 {
                    println!("! 切换备用源: {url}");
                }
                if let Some(batch) = batch {
                    batch.started(&label, url);
                }
                let official = *url == spec.url;
                match self.try_download(url, &spec.dest, spec.sha1.as_deref(), spec.size, batch) {
                    Ok(()) => {
                        if let Some(home) = &home {
                            if official {
                                clear_fallback(home);
                            } else if official_failed {
                                note_fallback(home, url);
                            }
                        }
                        if let Some(batch) = batch {
                            batch.file_done(0);
                        }
                        return Ok(());
                    }
                    Err(e @ Error::Cancelled) => return Err(e),
                    Err(e) => {
                        // 校验不过不算「连不上」：换了源也该是同一个文件。
                        if official && !matches!(e, Error::Checksum { .. }) {
                            official_failed = true;
                        }
                        attempt_error = Some(e)
                    }
                }
            }
            if let (Some(batch), Some(before)) = (batch, before) {
                let after = batch.bytes_done.load(Ordering::Relaxed);
                batch
                    .bytes_done
                    .fetch_sub(after.saturating_sub(before), Ordering::Relaxed);
            }
            last = attempt_error;
            cancel::check()?;
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
        expected_size: Option<u64>,
        batch: Option<&Batch>,
    ) -> Result<()> {
        cancel::check()?;
        let resp = self
            .client
            .get(url)
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .map_err(|e| Error::Net {
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
        if let (Some(expected), Some(actual)) = (expected_size, resp.content_length())
            && actual != expected
        {
            return Err(Error::Install(format!(
                "文件大小不符: 期望 {expected}, 实际 {actual} ({url})"
            )));
        }
        let tmp = dest.with_extension(format!("part-{}", std::process::id()));
        let mut file = File::create(&tmp).map_err(|e| io(&tmp, e))?;
        let mut hasher = Sha1::new();
        let mut reader = resp;
        let mut buf = [0u8; 64 * 1024];
        loop {
            cancel::check()?;
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
        cancel::check()?;
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
        cancel::check()?;
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
            current: std::sync::Mutex::new((None, None)),
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
        cancel::check()?;
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
    cancel::check()?;
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
        cancel::check()?;
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
    /// 最近开始下载的文件和它的来源主机。
    current: std::sync::Mutex<(Option<String>, Option<String>)>,
}

/// `https://host/path` 里的 host。
fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    Some(rest.split(['/', '?']).next()?.to_string()).filter(|h| !h.is_empty())
}

impl Batch<'_> {
    fn started(&self, label: &str, url: &str) {
        if let Ok(mut current) = self.current.lock() {
            *current = (Some(label.to_string()), host_of(url));
        }
        self.emit(false);
    }

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
        let (current, source) = self
            .current
            .lock()
            .map(|current| current.clone())
            .unwrap_or_default();
        let progress = Progress {
            stage: self.stage.clone(),
            files_done: self.files_done.load(Ordering::Relaxed),
            files_total: self.files_total,
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total,
            indeterminate: false,
            current,
            source,
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

/// MCIM：Modrinth 和 CurseForge 接口的国内镜像（<https://github.com/mcmod-info-mirror>）。
pub const MCIM_HOST: &str = "mod.mcimirror.top";

/// 官方连不上之后，多久之内同一主机直接走后备。过了再试官方。
const FALLBACK_FOR_MS: u64 = 5 * 60 * 1000;

/// 一个地址能去哪些地方取，按先后排好：
///
/// - Mojang 和加载器的文件、清单：官方和 BMCLAPI 互为后备，`mirror_first` 时 BMCLAPI 在前；
/// - Modrinth 和 CurseForge 的接口：总是官方在前，连不上才问 MCIM——登记时答应过它只做后备。
///   MCIM 的文件地址目前只是跳回官方 CDN，所以文件不走它。
pub fn fallback_urls(original: &str, mirror_first: bool) -> Vec<String> {
    let mut out = vec![original.to_string()];
    let bmcl = rewrite_mirror(original, "bmcl");
    if bmcl != original {
        if mirror_first {
            out.insert(0, bmcl);
        } else {
            out.push(bmcl);
        }
    }
    if let Some(mcim) = mcim_url(original) {
        out.push(mcim);
    }
    out
}

fn mcim_url(url: &str) -> Option<String> {
    let rules = [
        (
            "https://api.modrinth.com/",
            "https://mod.mcimirror.top/modrinth/",
        ),
        (
            "https://api.curseforge.com/",
            "https://mod.mcimirror.top/curseforge/",
        ),
    ];
    rules
        .iter()
        .find_map(|(from, to)| url.strip_prefix(from).map(|rest| format!("{to}{rest}")))
}

/// 发给 `url` 的请求头。密钥和登录凭据只发给 `original` 原本的主机，换到镜像就去掉。
fn headers_for<'a>(
    headers: &'a [(&'a str, &'a str)],
    original: &str,
    url: &str,
) -> impl Iterator<Item = &'a (&'a str, &'a str)> {
    let same_host = host_of(original) == host_of(url);
    headers.iter().filter(move |(name, _)| {
        same_host
            || !["x-api-key", "authorization", "cookie"]
                .iter()
                .any(|secret| name.eq_ignore_ascii_case(secret))
    })
}

/// 官方给出这些答案时，换镜像也不会更好。403 和 401 不算：常是地区限制或密钥问题，镜像可能帮得上。
fn is_final_status(status: u16) -> bool {
    matches!(status, 400 | 404 | 410 | 422)
}

/// 官方连不上、正在用镜像的一个主机。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    /// 换到的主机，例如 `bmclapi2.bangbang93.com`。
    pub via: String,
    /// 镜像数据最晚什么时候同步过（Unix 秒）。只有 MCIM 会告诉我们。
    pub synced: Option<u64>,
}

static FALLBACKS: std::sync::Mutex<Vec<(String, String, u64)>> = std::sync::Mutex::new(Vec::new());
static MCIM_SYNCED: AtomicU64 = AtomicU64::new(0);

/// `host`（官方主机，例如 `api.modrinth.com`）最近连不上、正在用镜像时，返回用的是哪个。
pub fn fallback_for(host: &str) -> Option<Fallback> {
    let now = now_ms();
    let list = FALLBACKS.lock().ok()?;
    let (_, via, _) = list
        .iter()
        .find(|(home, _, until)| home == host && *until > now)?;
    let synced = (via == MCIM_HOST)
        .then(|| MCIM_SYNCED.load(Ordering::Relaxed))
        .filter(|secs| *secs > 0);
    Some(Fallback {
        via: via.clone(),
        synced,
    })
}

fn note_fallback(home: &str, used: &str) {
    let Some(via) = host_of(used) else {
        return;
    };
    let until = now_ms() + FALLBACK_FOR_MS;
    if let Ok(mut list) = FALLBACKS.lock() {
        list.retain(|(host, _, _)| host != home);
        list.push((home.to_string(), via, until));
    }
}

fn clear_fallback(home: &str) {
    if let Ok(mut list) = FALLBACKS.lock() {
        list.retain(|(host, _, _)| host != home);
    }
}

/// MCIM 的每个对象都带 `sync_at`。记下这次回答里最旧的那个：数据最多旧到这里。
fn note_synced(body: &str) {
    let oldest = body
        .split("\"sync_at\":\"")
        .skip(1)
        .filter_map(|rest| parse_utc(rest.split('"').next()?))
        .min();
    if let Some(oldest) = oldest {
        MCIM_SYNCED.store(oldest, Ordering::Relaxed);
    }
}

/// `2026-11-06T21:30:02.552Z` → Unix 秒。只认 UTC。
fn parse_utc(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let mut time = time.splitn(3, ':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next()?.parse::<i64>().ok()?;
    let second = time.next()?.get(..2)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // 公历日期到天数（Howard Hinnant 的算法）。
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
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
            "https://authlib-injector.yushi.moe",
            "https://bmclapi2.bangbang93.com/mirrors/authlib-injector",
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
    fn reads_the_host_of_a_url() {
        assert_eq!(
            host_of("https://piston-data.mojang.com/v1/objects/abc/client.jar").as_deref(),
            Some("piston-data.mojang.com")
        );
        assert_eq!(
            host_of("https://bmclapi2.bangbang93.com?x=1").as_deref(),
            Some("bmclapi2.bangbang93.com")
        );
        assert_eq!(host_of("not a url"), None);
    }

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
    fn mod_platforms_fall_back_to_mcim_only_after_official() {
        let search = "https://api.modrinth.com/v2/search?query=sodium";
        for mirror_first in [false, true] {
            assert_eq!(
                fallback_urls(search, mirror_first),
                [
                    search.to_string(),
                    "https://mod.mcimirror.top/modrinth/v2/search?query=sodium".to_string()
                ]
            );
        }
        assert_eq!(
            fallback_urls("https://api.curseforge.com/v1/mods/238222", true)[1],
            "https://mod.mcimirror.top/curseforge/v1/mods/238222"
        );
        // MCIM 的文件地址只会跳回官方 CDN，文件不走它。
        let file = "https://cdn.modrinth.com/data/AANobbMI/versions/x/sodium.jar";
        assert_eq!(fallback_urls(file, false), [file.to_string()]);
        // BMCLAPI 只管 Mojang 和加载器，不碰 Mod 平台。
        let client = "https://piston-data.mojang.com/v1/objects/abc/client.jar";
        assert_eq!(fallback_urls(client, false).len(), 2);
        assert!(fallback_urls(client, true)[0].contains("bmclapi"));
    }

    #[test]
    fn api_keys_stay_with_their_own_host() {
        let headers = [("x-api-key", "secret"), ("Accept", "application/json")];
        let official = "https://api.curseforge.com/v1/mods/1";
        let sent: Vec<_> = headers_for(&headers, official, official).collect();
        assert_eq!(sent.len(), 2);
        let mirror = "https://mod.mcimirror.top/curseforge/v1/mods/1";
        let sent: Vec<_> = headers_for(&headers, official, mirror).collect();
        assert_eq!(sent, [&("Accept", "application/json")]);
        let upper = [("X-Api-Key", "secret"), ("Authorization", "Bearer x")];
        assert_eq!(headers_for(&upper, official, mirror).count(), 0);
    }

    #[test]
    fn reads_mcim_sync_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2026-11-06T21:30:02.552Z"), Some(1_794_000_602));
        assert_eq!(parse_utc("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(parse_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc("yesterday"), None);
        note_synced(
            r#"{"hits":[{"sync_at":"2026-11-06T21:30:02.552Z"},{"sync_at":"2024-02-29T23:59:59Z"}]}"#,
        );
        assert_eq!(MCIM_SYNCED.load(Ordering::Relaxed), 1_709_251_199);
    }

    #[test]
    fn a_fallback_is_remembered_until_official_works_again() {
        let home = "fallback-test.example";
        assert_eq!(fallback_for(home), None);
        note_fallback(home, "https://bmclapi2.bangbang93.com/x");
        assert_eq!(
            fallback_for(home).map(|f| f.via),
            Some("bmclapi2.bangbang93.com".to_string())
        );
        clear_fallback(home);
        assert_eq!(fallback_for(home), None);
    }

    #[test]
    fn cancelled_download_stops_before_completion() {
        use std::net::TcpListener;

        let _guard = crate::cancel::TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8388608\r\nConnection: close\r\n\r\n",
            );
            let chunk = [0u8; 65536];
            for _ in 0..128 {
                if stream.write_all(&chunk).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });

        cancel::reset();
        let root =
            std::env::temp_dir().join(format!("nailong-cancel-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let dest = root.join("large.bin");
        let http = Http::new("official").unwrap();
        let spec = FileSpec {
            url: format!("http://{addr}/large.bin"),
            dest: dest.clone(),
            sha1: None,
            size: Some(8 * 1024 * 1024),
            verify_existing: false,
            label: None,
        };
        let canceller = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(60));
            cancel::cancel();
        });
        let error = http.download(&spec).unwrap_err();
        canceller.join().unwrap();
        server.join().unwrap();
        cancel::reset();
        let _ = std::fs::remove_dir_all(&root);
        assert!(matches!(error, Error::Cancelled));
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
