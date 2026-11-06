//! 按版本下载 Java。
//!
//! 用的是 Mojang 官方启动器同一份运行时清单：每个平台、每个 Java 大版本一个组件
//! （`java-runtime-delta` 是 21，`jre-legacy` 是 8……），清单里列出每个文件的 SHA-1、
//! 大小和下载地址，多数文件还有一份 LZMA 压缩版，只有原始大小的三分之一左右。
//!
//! 装到 `<数据目录>/runtime/<组件>/`。先下到旁边的 `.part` 目录，全部校验通过、
//! 解压完、设好可执行权限后才改名过去，所以中途断了不会留下半个 Java。
//! 官方源不通时元数据和文件都会换 BMCLAPI 镜像。

use crate::cancel;
use crate::error::{Error, Result, io};
use crate::http::{FileSpec, Http, rewrite_mirror, sha1_file};
use crate::java;
use crate::paths::Root;
use crate::progress::{Progress, Sink};
use serde_json::Value;
use std::path::{Path, PathBuf};

const INDEX: &str = "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const VERSION_FILE: &str = ".cairn-runtime";

/// 运行时清单里这台电脑的平台名。
pub fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("windows", "x86") => Some("windows-x86"),
        ("windows", "aarch64") => Some("windows-arm64"),
        ("linux", "x86_64") => Some("linux"),
        ("linux", "x86") => Some("linux-i386"),
        ("macos", "x86_64") => Some("mac-os"),
        ("macos", "aarch64") => Some("mac-os-arm64"),
        _ => None,
    }
}

/// 可以下载的一个 Java。
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    /// 例如 `java-runtime-delta`。
    pub component: String,
    /// 例如 `21.0.7`。
    pub version: String,
    pub major: u32,
    manifest_url: String,
    manifest_sha1: Option<String>,
}

pub fn dir(root: &Root) -> PathBuf {
    root.dir().join("runtime")
}

/// 先试官方地址，不通再试镜像。元数据请求很小，失败得快。
fn get_text(http: &Http, url: &str) -> Result<String> {
    match http.get_text(url) {
        Ok(text) => Ok(text),
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(first) => {
            let mirror = rewrite_mirror(url, "bmcl");
            if mirror == url {
                return Err(first);
            }
            http.get_text(&mirror)
        }
    }
}

/// 从总清单里挑：版本里写明的组件优先；否则选够用里大版本最低的。
pub fn choose(
    index: &Value,
    platform: &str,
    required: u32,
    preferred: Option<&str>,
) -> Option<Offer> {
    let components = index.get(platform)?.as_object()?;
    let mut offers: Vec<Offer> = components
        .iter()
        .filter_map(|(component, list)| {
            let entry = list.as_array()?.first()?;
            let version = entry.pointer("/version/name")?.as_str()?.to_string();
            Some(Offer {
                component: component.clone(),
                major: java::major_from_version(&version),
                version,
                manifest_url: entry.pointer("/manifest/url")?.as_str()?.to_string(),
                manifest_sha1: entry
                    .pointer("/manifest/sha1")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            })
        })
        .filter(|offer| offer.major >= required)
        .collect();
    if let Some(preferred) = preferred
        && let Some(offer) = offers.iter().find(|o| o.component == preferred)
    {
        return Some(offer.clone());
    }
    offers.sort_by(|a, b| a.major.cmp(&b.major).then(b.version.cmp(&a.version)));
    offers.into_iter().next()
}

/// 查一下能下载哪个 Java。
pub fn find(http: &Http, required: u32, preferred: Option<&str>) -> Result<Offer> {
    let platform = platform()
        .ok_or_else(|| Error::Install("这个系统没有官方的 Java 运行时，需要自己安装".into()))?;
    let index: Value = serde_json::from_str(&get_text(http, INDEX)?)
        .map_err(|e| Error::Install(format!("Java 运行时清单解析失败: {e}")))?;
    choose(&index, platform, required, preferred)
        .ok_or_else(|| Error::Install(format!("官方没有给这个系统提供 Java {required}")))
}

/// 运行时清单里的一个文件。
#[derive(Debug, Clone, PartialEq)]
enum Entry {
    Dir(String),
    File {
        path: String,
        executable: bool,
        raw: (String, u64, String),
        lzma: Option<(String, u64, String)>,
    },
    Link {
        path: String,
        target: String,
    },
}

fn download_of(file: &Value, kind: &str) -> Option<(String, u64, String)> {
    let d = file.pointer(&format!("/downloads/{kind}"))?;
    Some((
        d.get("sha1")?.as_str()?.to_string(),
        d.get("size")?.as_u64()?,
        d.get("url")?.as_str()?.to_string(),
    ))
}

fn entries(manifest: &Value) -> Result<Vec<Entry>> {
    let files = manifest
        .get("files")
        .and_then(|f| f.as_object())
        .ok_or_else(|| Error::Install("Java 运行时清单缺少 files".into()))?;
    let mut out = Vec::new();
    for (path, file) in files {
        // 清单里的路径一律是相对路径；带 .. 的不要，免得写到运行时目录外面。
        if path.starts_with('/') || path.split('/').any(|part| part == "..") {
            continue;
        }
        match file.get("type").and_then(|t| t.as_str()) {
            Some("directory") => out.push(Entry::Dir(path.clone())),
            Some("file") => {
                let Some(raw) = download_of(file, "raw") else {
                    continue;
                };
                out.push(Entry::File {
                    path: path.clone(),
                    executable: file.get("executable").and_then(|e| e.as_bool()) == Some(true),
                    lzma: download_of(file, "lzma"),
                    raw,
                });
            }
            Some("link") => {
                if let Some(target) = file.get("target").and_then(|t| t.as_str()) {
                    out.push(Entry::Link {
                        path: path.clone(),
                        target: target.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// 运行时里 java 程序的位置：Windows 和 Linux 在 `bin/` 下，macOS 在 `jre.bundle` 里面。
pub fn java_in(home: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    [
        home.join("bin").join(exe),
        home.join("jre.bundle/Contents/Home/bin").join(exe),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// 已经装好的运行时里的 java 程序。
pub fn installed(root: &Root) -> Vec<PathBuf> {
    std::fs::read_dir(dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join(VERSION_FILE).is_file())
        .filter_map(|entry| java_in(&entry.path()))
        .collect()
}

/// 解压 LZMA 版、校验原始文件的 SHA-1、设可执行权限、建链接。
fn finish(staging: &Path, files: &[Entry], sink: &Sink) -> Result<()> {
    let total = files.len() as u64;
    for (index, entry) in files.iter().enumerate() {
        cancel::check()?;
        if index % 32 == 0 {
            (sink)(Progress {
                stage: "解压 Java".into(),
                files_done: index as u64,
                files_total: total,
                ..Default::default()
            });
        }
        match entry {
            Entry::Dir(path) => {
                let dir = staging.join(path);
                std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
            }
            Entry::File {
                path,
                executable,
                raw,
                lzma,
            } => {
                let dest = staging.join(path);
                let packed = dest.with_file_name(format!(
                    "{}.lzma",
                    dest.file_name().unwrap_or_default().to_string_lossy()
                ));
                if lzma.is_some() && packed.is_file() {
                    let input = std::fs::File::open(&packed).map_err(|e| io(&packed, e))?;
                    let output = std::fs::File::create(&dest).map_err(|e| io(&dest, e))?;
                    let mut reader = std::io::BufReader::new(input);
                    let mut writer = std::io::BufWriter::new(output);
                    lzma_rs::lzma_decompress(&mut reader, &mut writer)
                        .map_err(|e| Error::Install(format!("解压 {path} 失败: {e}")))?;
                    drop(writer);
                    let _ = std::fs::remove_file(&packed);
                    let actual = sha1_file(&dest)?;
                    if !actual.eq_ignore_ascii_case(&raw.0) {
                        return Err(Error::Install(format!("{path} 解压后校验不符")));
                    }
                }
                #[cfg(unix)]
                if *executable {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
                        .map_err(|e| io(&dest, e))?;
                }
                #[cfg(not(unix))]
                let _ = executable;
            }
            Entry::Link { path, target } => {
                #[cfg(unix)]
                {
                    let link = staging.join(path);
                    if let Some(parent) = link.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                    }
                    let _ = std::fs::remove_file(&link);
                    std::os::unix::fs::symlink(target, &link).map_err(|e| io(&link, e))?;
                }
                #[cfg(not(unix))]
                let _ = (path, target);
            }
        }
    }
    Ok(())
}

/// 下载并装好一个 Java，返回 java 程序的路径。已经装过同一个版本就直接返回。
pub fn install(root: &Root, http: &Http, offer: &Offer, sink: &Sink) -> Result<PathBuf> {
    cancel::check()?;
    let home = dir(root).join(&offer.component);
    if std::fs::read_to_string(home.join(VERSION_FILE))
        .ok()
        .as_deref()
        == Some(&offer.version)
        && let Some(java) = java_in(&home)
    {
        return Ok(java);
    }
    (sink)(Progress::stage(format!("准备下载 Java {}", offer.major)));
    let text = get_text(http, &offer.manifest_url)?;
    if let Some(expected) = &offer.manifest_sha1 {
        use sha1::{Digest, Sha1};
        let actual = crate::http::hex(&Sha1::digest(text.as_bytes()));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::Install(
                "Java 运行时清单校验不符，可能被篡改或下载不完整".into(),
            ));
        }
    }
    let manifest: Value = serde_json::from_str(&text)
        .map_err(|e| Error::Install(format!("Java 运行时清单解析失败: {e}")))?;
    let files = entries(&manifest)?;

    let staging = dir(root).join(format!("{}.part", offer.component));
    std::fs::create_dir_all(&staging).map_err(|e| io(&staging, e))?;
    let specs: Vec<FileSpec> = files
        .iter()
        .filter_map(|entry| match entry {
            Entry::File {
                path, raw, lzma, ..
            } => {
                let dest = staging.join(path);
                // 有压缩版而且更小就下压缩版。
                let (sha1, size, url, dest) = match lzma {
                    Some(packed) if packed.1 < raw.1 => (
                        packed.0.clone(),
                        packed.1,
                        packed.2.clone(),
                        dest.with_file_name(format!(
                            "{}.lzma",
                            dest.file_name().unwrap_or_default().to_string_lossy()
                        )),
                    ),
                    _ => (raw.0.clone(), raw.1, raw.2.clone(), dest),
                };
                Some(FileSpec {
                    url,
                    dest,
                    sha1: Some(sha1),
                    size: Some(size),
                    verify_existing: true,
                    label: Some(path.clone()),
                })
            }
            _ => None,
        })
        .collect();
    // 已经解压过的原始文件就不再下压缩版了（上次解压到一半断掉的情况）。
    let specs: Vec<FileSpec> = specs
        .into_iter()
        .filter(|spec| {
            let raw = spec.dest.with_extension("");
            !(spec.dest.extension().is_some_and(|e| e == "lzma") && raw.is_file())
        })
        .collect();
    http.download_many_with(specs, &format!("下载 Java {}", offer.major), sink)?;
    finish(&staging, &files, sink)?;
    std::fs::write(staging.join(VERSION_FILE), &offer.version).map_err(|e| io(&staging, e))?;
    if home.exists() {
        std::fs::remove_dir_all(&home).map_err(|e| io(&home, e))?;
    }
    std::fs::rename(&staging, &home).map_err(|e| io(&home, e))?;
    (sink)(Progress {
        stage: format!("Java {} 已装好", offer.major),
        files_done: 1,
        files_total: 1,
        ..Default::default()
    });
    java_in(&home).ok_or_else(|| Error::Install("运行时里找不到 java 程序".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn index() -> Value {
        serde_json::json!({
            "linux": {
                "java-runtime-alpha": [{"manifest": {"url": "https://x/a.json", "sha1": "a"}, "version": {"name": "16.0.1.9.1"}}],
                "java-runtime-gamma": [{"manifest": {"url": "https://x/g.json", "sha1": "g"}, "version": {"name": "17.0.8"}}],
                "java-runtime-delta": [{"manifest": {"url": "https://x/d.json", "sha1": "d"}, "version": {"name": "21.0.7"}}],
                "java-runtime-epsilon": [{"manifest": {"url": "https://x/e.json", "sha1": "e"}, "version": {"name": "25.0.1"}}],
                "jre-legacy": [{"manifest": {"url": "https://x/l.json", "sha1": "l"}, "version": {"name": "8u51"}}],
                "minecraft-java-exe": []
            }
        })
    }

    #[test]
    fn picks_the_lowest_sufficient_runtime_or_the_named_one() {
        let index = index();
        assert_eq!(
            choose(&index, "linux", 21, None).unwrap().component,
            "java-runtime-delta"
        );
        assert_eq!(choose(&index, "linux", 17, None).unwrap().major, 17);
        assert_eq!(choose(&index, "linux", 22, None).unwrap().major, 25);
        assert_eq!(
            choose(&index, "linux", 8, None).unwrap().component,
            "jre-legacy"
        );
        assert_eq!(
            choose(&index, "linux", 21, Some("java-runtime-epsilon"))
                .unwrap()
                .major,
            25
        );
        // 指定的组件不够用时不要它。
        assert_eq!(
            choose(&index, "linux", 21, Some("java-runtime-gamma"))
                .unwrap()
                .major,
            21
        );
        assert!(choose(&index, "linux", 26, None).is_none());
        assert!(choose(&index, "windows-x64", 21, None).is_none());
    }

    #[test]
    fn reads_files_and_refuses_paths_outside() {
        let manifest = serde_json::json!({"files": {
            "bin": {"type": "directory"},
            "bin/java": {"type": "file", "executable": true, "downloads": {
                "raw": {"sha1": "r", "size": 10, "url": "https://x/raw"},
                "lzma": {"sha1": "z", "size": 4, "url": "https://x/lzma"}}},
            "legal/LICENSE": {"type": "link", "target": "../LICENSE"},
            "../evil": {"type": "file", "downloads": {"raw": {"sha1": "e", "size": 1, "url": "https://x/e"}}}
        }});
        let files = entries(&manifest).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.contains(&Entry::Dir("bin".into())));
        assert!(files.iter().any(|f| matches!(f, Entry::File { path, executable: true, lzma: Some(_), .. } if path == "bin/java")));
    }

    #[test]
    fn unpacks_verifies_and_marks_executables() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let staging =
            std::env::temp_dir().join(format!("cairn-runtime-{}-{n}", std::process::id()));
        std::fs::create_dir_all(staging.join("bin")).unwrap();
        let content = b"#!/bin/sh\necho java\n".repeat(20);
        let mut packed = Vec::new();
        lzma_rs::lzma_compress(&mut &content[..], &mut packed).unwrap();
        std::fs::File::create(staging.join("bin/java.lzma"))
            .unwrap()
            .write_all(&packed)
            .unwrap();
        use sha1::{Digest, Sha1};
        let raw_sha1 = crate::http::hex(&Sha1::digest(&content));
        let files = vec![
            Entry::Dir("bin".into()),
            Entry::File {
                path: "bin/java".into(),
                executable: true,
                raw: (raw_sha1, content.len() as u64, String::new()),
                lzma: Some(("x".into(), packed.len() as u64, String::new())),
            },
            Entry::Link {
                path: "lib/java".into(),
                target: "../bin/java".into(),
            },
        ];
        finish(&staging, &files, &crate::progress::noop()).unwrap();
        assert_eq!(std::fs::read(staging.join("bin/java")).unwrap(), content);
        assert!(!staging.join("bin/java.lzma").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(staging.join("bin/java"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
            assert!(staging.join("lib/java").exists());
        }
        // 解压后校验不符要报错。
        let mut bad = files.clone();
        if let Entry::File { raw, .. } = &mut bad[1] {
            raw.0 = "0000".into();
        }
        std::fs::File::create(staging.join("bin/java.lzma"))
            .unwrap()
            .write_all(&packed)
            .unwrap();
        assert!(finish(&staging, &bad, &crate::progress::noop()).is_err());
        assert_eq!(java_in(&staging), Some(staging.join("bin/java")));
        let _ = std::fs::remove_dir_all(&staging);
    }
}
