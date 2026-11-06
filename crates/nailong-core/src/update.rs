//! 启动器自己的更新：查 GitHub 上的最新版本，下载这个系统用的文件，按 SHA256SUMS 校验。
//!
//! - Windows：下载新的 `cairn.exe`，把正在运行的程序改名成 `cairn.old.exe`（Windows 允许给
//!   运行中的 exe 改名），新程序放到原位置再启动，下次启动时删掉旧的。
//! - Linux：deb 需要管理员权限安装，所以只下载到「下载」目录，给出一条能直接复制的命令。

use crate::error::{Error, Result, io};
use crate::http::{FileSpec, Http};
use crate::progress::Sink;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const REPO: &str = "dkjsiogu/cairn";

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// 不带 v，例如 `0.38.0`。
    pub version: String,
    /// 发布说明（Markdown 原文）。
    pub notes: String,
    /// 发布页面。
    pub page: String,
    pub assets: Vec<Asset>,
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_release(value: &Value) -> Option<Release> {
    let tag = value.get("tag_name")?.as_str()?;
    let assets = value
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|asset| {
            Some(Asset {
                name: asset.get("name")?.as_str()?.to_string(),
                url: asset.get("browser_download_url")?.as_str()?.to_string(),
                size: asset.get("size")?.as_u64()?,
            })
        })
        .collect();
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        notes: value
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        page: value
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string(),
        assets,
    })
}

/// GitHub 上标为最新的正式版。
pub fn latest(http: &Http) -> Result<Release> {
    let value = http.get_json_with_headers(
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        &[("Accept", "application/vnd.github+json")],
    )?;
    parse_release(&value).ok_or_else(|| Error::Install("读不懂 GitHub 返回的版本信息".into()))
}

fn numbers(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .map_while(|part| part.parse().ok())
        .collect()
}

/// `latest` 比 `current` 新。只比较数字部分，`0.38.0` > `0.37.2`。
pub fn is_newer(latest: &str, current: &str) -> bool {
    numbers(latest) > numbers(current)
}

/// 这个系统该下哪个文件。
pub fn asset_for_this_system(release: &Release) -> Option<&Asset> {
    let wanted = |name: &str| {
        if cfg!(windows) {
            name.eq_ignore_ascii_case("cairn.exe")
        } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            name.starts_with("cairn_") && name.ends_with("_amd64.deb")
        } else {
            false
        }
    };
    release.assets.iter().find(|asset| wanted(&asset.name))
}

fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| io(path, e))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| io(path, e))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(crate::http::hex(&hash.finalize()))
}

/// SHA256SUMS 里某个文件的值（`<hash>  <name>` 或 `<hash>  ./<name>`）。
fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*').trim_start_matches("./");
        (file == name).then(|| hash.to_ascii_lowercase())
    })
}

/// 下载到 `dir`，按发布里的 SHA256SUMS 校验，返回文件路径。
pub fn download(
    http: &Http,
    release: &Release,
    asset: &Asset,
    dir: &Path,
    sink: &Sink,
) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let sums = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or_else(|| Error::Install("这个版本没有发布校验值，不安全，没有下载".into()))?;
    let expected = expected_sha256(&http.get_text(&sums.url)?, &asset.name)
        .ok_or_else(|| Error::Install(format!("校验值里没有 {}", asset.name)))?;
    let dest = dir.join(&asset.name);
    let _ = std::fs::remove_file(&dest);
    http.download_with(
        FileSpec {
            url: asset.url.clone(),
            dest: dest.clone(),
            sha1: None,
            size: Some(asset.size),
            verify_existing: false,
            label: Some(asset.name.clone()),
        },
        &format!("下载 Cairn {}", release.version),
        sink,
    )?;
    let actual = sha256_file(&dest)?;
    if actual != expected {
        let _ = std::fs::remove_file(&dest);
        return Err(Error::Install(
            "下载的文件和发布的校验值对不上，已删除；请稍后重试".into(),
        ));
    }
    Ok(dest)
}

/// 上次更新留下的旧程序。
fn old_exe(current: &Path) -> PathBuf {
    current.with_file_name("cairn.old.exe")
}

/// 启动时调用：删掉上次更新留下的旧程序。
pub fn cleanup() {
    if let Ok(current) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe(&current));
    }
}

/// Windows：用新程序替换正在运行的程序，再启动新的。调用方随后应退出。
pub fn replace_running_exe(new_exe: &Path) -> Result<()> {
    if !cfg!(windows) {
        return Err(Error::Install("只有 Windows 能自动替换程序".into()));
    }
    let current = std::env::current_exe().map_err(|e| Error::Install(e.to_string()))?;
    let old = old_exe(&current);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&current, &old).map_err(|e| io(&current, e))?;
    if let Err(e) = std::fs::copy(new_exe, &current) {
        // 放不进去就把旧程序改回来，至少还能用。
        let _ = std::fs::rename(&old, &current);
        return Err(io(&current, e));
    }
    std::process::Command::new(&current)
        .spawn()
        .map_err(|e| Error::Install(format!("新版本没能启动：{e}")))?;
    Ok(())
}

/// 下载新版本放在哪：Windows 放数据目录的缓存里（马上就替换），Linux 放「下载」目录（要手动装）。
pub fn download_dir(cache: &Path) -> PathBuf {
    if cfg!(windows) {
        return cache.join("update");
    }
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .unwrap_or_else(|| cache.join("update"))
}

/// Linux 上装 deb 的命令，给玩家复制。
pub fn install_command(deb: &Path) -> String {
    format!("sudo apt install '{}'", deb.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_by_number() {
        assert!(is_newer("0.38.0", "0.37.2"));
        assert!(is_newer("v0.37.10", "0.37.9"));
        assert!(is_newer("1.0.0", "0.99.9"));
        assert!(!is_newer("0.37.0", "0.37.0"));
        assert!(!is_newer("0.36.9", "0.37.0"));
    }

    #[test]
    fn reads_a_github_release() {
        let value = serde_json::json!({
            "tag_name": "v0.38.0",
            "html_url": "https://github.com/dkjsiogu/cairn/releases/tag/v0.38.0",
            "body": "改了很多",
            "assets": [
                {"name": "cairn.exe", "browser_download_url": "https://x/cairn.exe", "size": 9000000},
                {"name": "cairn_0.38.0_amd64.deb", "browser_download_url": "https://x/c.deb", "size": 4000000},
                {"name": "SHA256SUMS", "browser_download_url": "https://x/SHA256SUMS", "size": 200}
            ]
        });
        let release = parse_release(&value).unwrap();
        assert_eq!(release.version, "0.38.0");
        assert_eq!(release.assets.len(), 3);
        let asset = asset_for_this_system(&release);
        if cfg!(windows) {
            assert_eq!(asset.unwrap().name, "cairn.exe");
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(asset.unwrap().name, "cairn_0.38.0_amd64.deb");
        }
    }

    #[test]
    fn finds_the_checksum_for_a_file() {
        let sums = "ABC123  cairn.exe\ndef456  ./cairn_0.38.0_amd64.deb\n";
        assert_eq!(
            expected_sha256(sums, "cairn.exe").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            expected_sha256(sums, "cairn_0.38.0_amd64.deb").as_deref(),
            Some("def456")
        );
        assert_eq!(expected_sha256(sums, "other"), None);
    }
}
