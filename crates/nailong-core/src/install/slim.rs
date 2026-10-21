use crate::error::{Error, Result, io};
use crate::http::{FileSpec, Http};
use crate::paths::Root;
use crate::progress::{Progress, Sink};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

const EOCD_MAX: u64 = 65_557;
const MAX_ENTRY: u64 = 128 * 1024 * 1024;
const PARALLEL_RANGE: u64 = 1024 * 1024;
const RANGE_PARTS: u64 = 4;

/// Fetch only the data a modern Forge/NeoForge install needs.
///
/// The official installer is a 6–9 MB zip, but a client install uses just
/// `install_profile.json`, `version.json` and the client binary patch.  Read
/// those entries with HTTP ranges, cache a small zip, and never keep the full
/// installer.  Servers that do not implement ranges (some mirrors) fall back
/// to the official Maven, then to a normal full download.
pub fn prepare(
    root: &Root,
    url: &str,
    file: &str,
    mirror: &str,
    label: &str,
    sink: &Sink,
) -> Result<PathBuf> {
    let dir = root.cache().join("installers");
    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let slim = dir.join(slim_name(file));
    let full = dir.join(file);

    if slim.exists() {
        if slim_is_complete(&slim) {
            return Ok(slim);
        }
        let _ = std::fs::remove_file(&slim);
    }
    if full.exists() {
        match build_slim_from_zip(&full, &slim) {
            Ok(true) => {
                let _ = std::fs::remove_file(&full);
                return Ok(slim);
            }
            Ok(false) => return Ok(full),
            Err(e) => {
                println!("! 旧安装包无法读取，重新获取: {e}");
                let _ = std::fs::remove_file(&full);
            }
        }
    }

    (sink)(Progress::stage(format!("读取 {label} 安装信息")));
    let mut last_error = None;
    let mut needs_full = false;
    if let Ok(http) = Http::new(mirror) {
        match range_slim(&http, url, &slim, label, sink) {
            Ok(true) => return Ok(slim),
            Ok(false) => needs_full = true,
            Err(e) => last_error = Some(e),
        }
    }
    // Some Chinese mirrors answer Range with a normal 200 response.  Try the
    // official host for the 3 MB core before giving up on the lightweight path.
    if !needs_full
        && mirror != "official"
        && let Ok(http) = Http::new("official")
    {
        match range_slim(&http, url, &slim, label, sink) {
            Ok(true) => {
                println!("→ 当前镜像不支持分段读取，已从官方源只取安装所需数据");
                return Ok(slim);
            }
            Ok(false) => {}
            Err(e) => last_error = Some(e),
        }
    }

    if let Some(e) = &last_error {
        println!("! 精简读取失败，回退完整安装包: {e}");
    }
    (sink)(Progress::stage(format!("下载 {label} 完整安装包")));
    let http = Http::new(mirror)?;
    // A stale partial full jar must not be treated as complete.
    let _ = std::fs::remove_file(&full);
    http.download_with(
        FileSpec {
            url: url.to_string(),
            dest: full.clone(),
            sha1: None,
            size: None,
            verify_existing: false,
        },
        &format!("下载 {label} 完整安装包"),
        sink,
    )?;
    match build_slim_from_zip(&full, &slim) {
        Ok(true) => {
            let _ = std::fs::remove_file(&full);
            Ok(slim)
        }
        Ok(false) => Ok(full),
        Err(e) => {
            println!("! 无法生成精简安装包，保留完整安装包: {e}");
            Ok(full)
        }
    }
}

fn slim_name(file: &str) -> String {
    let name = Path::new(file)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(file);
    format!("{}.slim.zip", name.trim_end_matches(".jar"))
}

fn range_slim(http: &Http, url: &str, dest: &Path, label: &str, sink: &Sink) -> Result<bool> {
    let total = http
        .range_len(url)?
        .ok_or_else(|| Error::Unsupported(format!("服务器不支持按需读取: {url}")))?;
    if total < 22 {
        return Err(Error::Install(format!("安装包太小: {url}")));
    }

    let tail_start = total.saturating_sub(EOCD_MAX);
    let tail = required_range(http, url, tail_start, total - 1)?;
    let eocd = find_eocd(&tail, total)?;
    if eocd.entries == 0 {
        return Err(Error::Install("安装包是空的".into()));
    }
    if eocd.cd_size > MAX_ENTRY {
        return Err(Error::Install("安装包目录异常大".into()));
    }

    let central = if eocd.cd_offset >= tail_start && eocd.cd_offset + eocd.cd_size <= total {
        let start = (eocd.cd_offset - tail_start) as usize;
        let end = start + eocd.cd_size as usize;
        tail[start..end].to_vec()
    } else {
        required_range(http, url, eocd.cd_offset, eocd.cd_offset + eocd.cd_size - 1)?
    };
    let entries = parse_central_directory(&central)?;

    let profile_entry = entries
        .get("install_profile.json")
        .ok_or_else(|| Error::Install("安装包缺少 install_profile.json".into()))?;
    let profile_bytes = fetch_entry(http, url, profile_entry, total)?;
    let profile: Value = serde_json::from_slice(&profile_bytes)
        .map_err(|e| Error::Install(format!("install_profile.json 解析失败: {e}")))?;
    if client_uses_installer(&profile) {
        return Ok(false);
    }
    let needed = needed_entries(&profile)?;

    let mut total_bytes = 0u64;
    for name in &needed {
        if let Some(entry) = entries.get(name) {
            total_bytes = total_bytes.saturating_add(entry.compressed);
        }
    }
    let files_total = needed.len() as u64;
    let mut done = 0u64;
    let mut files = Vec::with_capacity(needed.len());
    for (index, name) in needed.into_iter().enumerate() {
        let bytes = if name == "install_profile.json" {
            profile_bytes.clone()
        } else {
            let entry = entries
                .get(&name)
                .ok_or_else(|| Error::Install(format!("安装包缺少 {name}")))?;
            fetch_entry(http, url, entry, total)?
        };
        done = done.saturating_add(entries.get(&name).map(|e| e.compressed).unwrap_or(0));
        files.push((name, bytes));
        (sink)(Progress {
            stage: format!("下载 {label} 安装数据"),
            files_done: (index + 1) as u64,
            files_total,
            bytes_done: done,
            bytes_total: total_bytes,
            indeterminate: false,
        });
    }
    write_slim(dest, &files)?;
    Ok(true)
}

fn required_range(http: &Http, url: &str, start: u64, end: u64) -> Result<Vec<u8>> {
    http.get_range(url, start, end)?
        .map(|response| response.bytes)
        .ok_or_else(|| Error::Unsupported(format!("服务器不支持按需读取: {url}")))
}

#[derive(Debug, Clone)]
struct Eocd {
    entries: u16,
    cd_size: u64,
    cd_offset: u64,
}

fn find_eocd(tail: &[u8], total: u64) -> Result<Eocd> {
    let signature = b"PK\x05\x06";
    let mut end = tail.len();
    while let Some(pos) = rfind(&tail[..end], signature) {
        if pos + 22 > tail.len() {
            end = pos;
            continue;
        }
        let comment_len = u16_at(tail, pos + 20) as usize;
        if pos + 22 + comment_len != tail.len() {
            end = pos;
            continue;
        }
        let disk = u16_at(tail, pos + 4);
        let cd_disk = u16_at(tail, pos + 6);
        let entries = u16_at(tail, pos + 10);
        let cd_size = u32_at(tail, pos + 12) as u64;
        let cd_offset = u32_at(tail, pos + 16) as u64;
        if disk != 0 || cd_disk != 0 {
            return Err(Error::Unsupported("不支持分卷 ZIP 安装包".into()));
        }
        if entries == u16::MAX || cd_size == u32::MAX as u64 || cd_offset == u32::MAX as u64 {
            return Err(Error::Unsupported("不支持 ZIP64 安装包".into()));
        }
        if cd_offset + cd_size > total {
            return Err(Error::Install("安装包目录越界".into()));
        }
        return Ok(Eocd {
            entries,
            cd_size,
            cd_offset,
        });
    }
    Err(Error::Install("找不到 ZIP 中央目录".into()))
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .rev()
        .find(|&i| &haystack[i..i + needle.len()] == needle)
}

#[derive(Debug, Clone)]
struct CentralEntry {
    method: u16,
    crc: u32,
    compressed: u64,
    uncompressed: u64,
    local_offset: u64,
}

fn parse_central_directory(data: &[u8]) -> Result<BTreeMap<String, CentralEntry>> {
    let mut entries = BTreeMap::new();
    let mut pos = 0usize;
    while pos + 46 <= data.len() {
        if u32_at(data, pos) != 0x0201_4b50 {
            break;
        }
        let flags = u16_at(data, pos + 8);
        let method = u16_at(data, pos + 10);
        let crc = u32_at(data, pos + 16);
        let compressed = u32_at(data, pos + 20) as u64;
        let uncompressed = u32_at(data, pos + 24) as u64;
        let name_len = u16_at(data, pos + 28) as usize;
        let extra_len = u16_at(data, pos + 30) as usize;
        let comment_len = u16_at(data, pos + 32) as usize;
        let local_offset = u32_at(data, pos + 42) as u64;
        let name_end = pos + 46 + name_len;
        if name_end + extra_len + comment_len > data.len() {
            return Err(Error::Install("安装包目录条目越界".into()));
        }
        if flags & 1 != 0 {
            return Err(Error::Unsupported("安装包已加密".into()));
        }
        if compressed == u32::MAX as u64
            || uncompressed == u32::MAX as u64
            || local_offset == u32::MAX as u64
        {
            return Err(Error::Unsupported("不支持 ZIP64 条目".into()));
        }
        if compressed > MAX_ENTRY || uncompressed > MAX_ENTRY {
            return Err(Error::Install(format!(
                "安装包条目过大（压缩 {compressed} / 解压 {uncompressed} 字节）"
            )));
        }
        if let Ok(name) = std::str::from_utf8(&data[pos + 46..name_end]) {
            entries.insert(
                name.to_string(),
                CentralEntry {
                    method,
                    crc,
                    compressed,
                    uncompressed,
                    local_offset,
                },
            );
        }
        pos = name_end + extra_len + comment_len;
    }
    if entries.is_empty() {
        return Err(Error::Install("安装包目录为空".into()));
    }
    Ok(entries)
}

fn fetch_entry(http: &Http, url: &str, entry: &CentralEntry, total: u64) -> Result<Vec<u8>> {
    let header_end = entry.local_offset.saturating_add(29);
    if header_end >= total {
        return Err(Error::Install("安装包条目越界".into()));
    }
    let header = required_range(http, url, entry.local_offset, header_end)?;
    if u32_at(&header, 0) != 0x0403_4b50 {
        return Err(Error::Install("安装包条目头损坏".into()));
    }
    let name_len = u16_at(&header, 26) as u64;
    let extra_len = u16_at(&header, 28) as u64;
    let data_start = entry.local_offset + 30 + name_len + extra_len;
    let data_end = data_start
        .checked_add(entry.compressed)
        .ok_or_else(|| Error::Install("安装包条目长度溢出".into()))?;
    if data_end > total {
        return Err(Error::Install("安装包条目数据越界".into()));
    }
    let raw = if entry.compressed == 0 {
        Vec::new()
    } else if entry.compressed >= PARALLEL_RANGE {
        // Large entries (the 3 MB client patch) are fetched in parallel
        // ranges.  This is still one logical ZIP entry, so CRC/length checks
        // below validate the assembled result.
        let chunk = entry.compressed.div_ceil(RANGE_PARTS);
        let parts: Vec<Result<Vec<u8>>> = (0..RANGE_PARTS)
            .into_par_iter()
            .map(|index| {
                let start = data_start + index * chunk;
                if start >= data_end {
                    return Ok(Vec::new());
                }
                let end = (start + chunk - 1).min(data_end - 1);
                required_range(http, url, start, end)
            })
            .collect();
        let mut out = Vec::with_capacity(entry.compressed as usize);
        for part in parts {
            out.extend_from_slice(&part?);
        }
        out
    } else {
        required_range(http, url, data_start, data_end - 1)?
    };
    let bytes = match entry.method {
        0 => raw,
        8 => {
            let mut decoder = flate2::read::DeflateDecoder::new(&raw[..]);
            let mut out = Vec::with_capacity(entry.uncompressed as usize);
            decoder
                .read_to_end(&mut out)
                .map_err(|e| Error::Install(format!("解压安装包条目失败: {e}")))?;
            out
        }
        other => {
            return Err(Error::Unsupported(format!("不支持的 ZIP 压缩方式 {other}")));
        }
    };
    if bytes.len() as u64 != entry.uncompressed {
        return Err(Error::Install("安装包条目解压长度不符".into()));
    }
    if crc32fast::hash(&bytes) != entry.crc {
        return Err(Error::Install("安装包条目校验失败".into()));
    }
    Ok(bytes)
}

fn client_uses_installer(profile: &Value) -> bool {
    profile
        .get("processors")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(
            |processor| match processor.get("sides").and_then(|v| v.as_array()) {
                Some(sides) => sides.iter().any(|s| s.as_str() == Some("client")),
                None => true,
            },
        )
        .any(|processor| {
            processor
                .get("args")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .any(|arg| arg.as_str().is_some_and(|a| a.contains("{INSTALLER}")))
        })
}

fn needed_entries(profile: &Value) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    names.insert("install_profile.json".to_string());
    let version_name = profile
        .get("json")
        .and_then(|v| v.as_str())
        .unwrap_or("version.json");
    names.insert(safe_entry_name(version_name)?);
    if let Some(data) = profile.get("data").and_then(|v| v.as_object()) {
        for value in data.values() {
            let Some(raw) = value.get("client").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(name) = raw.strip_prefix('/') {
                names.insert(safe_entry_name(name)?);
            }
        }
    }
    Ok(names.into_iter().collect())
}

fn safe_entry_name(name: &str) -> Result<String> {
    let name = name.trim_start_matches('/');
    if name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::Install(format!("安装包路径不安全: {name}")));
    }
    Ok(name.to_string())
}

fn write_slim(dest: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    let tmp = temp_path(dest);
    let result = (|| -> Result<()> {
        let file = File::create(&tmp).map_err(|e| io(&tmp, e))?;
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        for (name, bytes) in files {
            writer
                .start_file(name, options)
                .map_err(|e| Error::Install(format!("写入精简安装包失败: {e}")))?;
            writer
                .write_all(bytes)
                .map_err(|e| Error::Install(format!("写入精简安装包失败: {e}")))?;
        }
        let file = writer
            .finish()
            .map_err(|e| Error::Install(format!("完成精简安装包失败: {e}")))?;
        file.sync_all().map_err(|e| io(&tmp, e))?;
        std::fs::rename(&tmp, dest).map_err(|e| io(dest, e))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn build_slim_from_zip(source: &Path, dest: &Path) -> Result<bool> {
    let file = File::open(source).map_err(|e| io(source, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Install(format!("打开 {} 失败: {e}", source.display())))?;
    let profile_bytes = read_zip_entry(&mut zip, "install_profile.json")?;
    let profile: Value = serde_json::from_slice(&profile_bytes)
        .map_err(|e| Error::Install(format!("install_profile.json 解析失败: {e}")))?;
    if client_uses_installer(&profile) {
        return Ok(false);
    }
    let needed = needed_entries(&profile)?;
    let mut files = Vec::with_capacity(needed.len());
    for name in needed {
        let bytes = if name == "install_profile.json" {
            profile_bytes.clone()
        } else {
            read_zip_entry(&mut zip, &name)?
        };
        files.push((name, bytes));
    }
    write_slim(dest, &files)?;
    Ok(true)
}

fn read_zip_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| Error::Install(format!("安装包缺少 {name}: {e}")))?;
    if entry.size() > MAX_ENTRY {
        return Err(Error::Install(format!("安装包条目过大: {name}")));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Install(format!("读取 {name} 失败: {e}")))?;
    Ok(bytes)
}

fn slim_is_complete(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return false;
    };
    let Ok(profile_bytes) = read_zip_entry(&mut zip, "install_profile.json") else {
        return false;
    };
    let Ok(profile) = serde_json::from_slice::<Value>(&profile_bytes) else {
        return false;
    };
    if client_uses_installer(&profile) {
        return false;
    }
    let Ok(needed) = needed_entries(&profile) else {
        return false;
    };
    if !needed.iter().all(|name| zip.by_name(name).is_ok()) {
        return false;
    }
    let version_name = profile
        .get("json")
        .and_then(|v| v.as_str())
        .unwrap_or("version.json");
    let Ok(version_name) = safe_entry_name(version_name) else {
        return false;
    };
    let Ok(version_bytes) = read_zip_entry(&mut zip, &version_name) else {
        return false;
    };
    serde_json::from_slice::<Value>(&version_bytes)
        .ok()
        .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .is_some_and(|id| !id.is_empty())
}

fn temp_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("installer");
    dest.with_file_name(format!(".{name}.part-{}", std::process::id()))
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tiny_zip() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("install_profile.json", options).unwrap();
            writer
                .write_all(br#"{"json":"/version.json","data":{"BINPATCH":{"client":"/data/client.lzma"}}}"#)
                .unwrap();
            writer.start_file("version.json", options).unwrap();
            writer.write_all(br#"{"id":"test"}"#).unwrap();
            writer.start_file("data/client.lzma", options).unwrap();
            writer.write_all(b"patch").unwrap();
            writer.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn finds_and_parses_eocd() {
        let bytes = tiny_zip();
        let eocd = find_eocd(&bytes, bytes.len() as u64).unwrap();
        assert_eq!(eocd.entries, 3);
        let start = eocd.cd_offset as usize;
        let end = start + eocd.cd_size as usize;
        let entries = parse_central_directory(&bytes[start..end]).unwrap();
        assert!(entries.contains_key("install_profile.json"));
        assert!(entries.contains_key("data/client.lzma"));
    }

    #[test]
    fn needed_entries_include_only_client_data() {
        let profile: Value = serde_json::from_str(
            r#"{"json":"/version.json","data":{
                "BINPATCH":{"client":"/data/client.lzma","server":"/data/server.lzma"},
                "MC":{"client":"[net.minecraft:client:1.21.1]"}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            needed_entries(&profile).unwrap(),
            vec![
                "data/client.lzma".to_string(),
                "install_profile.json".to_string(),
                "version.json".to_string()
            ]
        );
    }

    #[test]
    fn detects_client_processors_that_need_the_full_installer() {
        let plain: Value = serde_json::from_str(
            r#"{"processors":[{"sides":["client"],"args":["--input","{MINECRAFT_JAR}"]}]}"#,
        )
        .unwrap();
        assert!(!client_uses_installer(&plain));
        let server: Value =
            serde_json::from_str(r#"{"processors":[{"sides":["server"],"args":["{INSTALLER}"]}]}"#)
                .unwrap();
        assert!(!client_uses_installer(&server));
        let client: Value = serde_json::from_str(
            r#"{"processors":[{"sides":["client"],"args":["--archive","{INSTALLER}"]}]}"#,
        )
        .unwrap();
        assert!(client_uses_installer(&client));
    }

    #[test]
    fn rejects_unsafe_entries() {
        assert!(safe_entry_name("../evil").is_err());
        assert!(safe_entry_name("/data/../../evil").is_err());
        assert!(safe_entry_name("data\\evil").is_err());
        assert_eq!(
            safe_entry_name("/data/client.lzma").unwrap(),
            "data/client.lzma"
        );
    }
}
