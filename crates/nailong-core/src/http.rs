use crate::error::{Error, Result, io};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use sha1::{Digest, Sha1};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
            .timeout(Duration::from_secs(60))
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

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value> {
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
                        return resp.json().map_err(|e| Error::Net {
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

    pub fn download(&self, spec: &FileSpec) -> Result<()> {
        if spec.dest.exists() {
            let size_ok = spec
                .size
                .is_none_or(|s| spec.dest.metadata().map(|m| m.len() == s).unwrap_or(false));
            if size_ok && !spec.verify_existing {
                return Ok(());
            }
            if size_ok {
                if let Some(expected) = &spec.sha1 {
                    let actual = sha1_file(&spec.dest)?;
                    if actual.eq_ignore_ascii_case(expected) {
                        return Ok(());
                    }
                    let _ = std::fs::remove_file(&spec.dest);
                } else {
                    return Ok(());
                }
            }
        }
        if let Some(parent) = spec.dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        let url = self.url(&spec.url);
        let mut last = None;
        for attempt in 0..3u32 {
            match self.try_download(&url, &spec.dest, spec.sha1.as_deref()) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(500 * (attempt + 1) as u64));
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::Install(format!("下载失败: {url}"))))
    }

    fn try_download(&self, url: &str, dest: &Path, expected: Option<&str>) -> Result<()> {
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
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| io(dest, e))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| io(&tmp, e))?;
            total += n as u64;
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
        let _ = total;
        std::fs::rename(&tmp, dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(dest, e)
        })?;
        Ok(())
    }

    pub fn download_many(&self, specs: Vec<FileSpec>, label: &str) -> Result<usize> {
        if specs.is_empty() {
            return Ok(0);
        }
        let bar = Arc::new(ProgressBar::new(specs.len() as u64));
        bar.set_style(
            ProgressStyle::with_template("{spinner:.green} {msg} {pos}/{len} [{bar:30}]").unwrap(),
        );
        bar.set_message(label.to_string());
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(16)
            .build()
            .map_err(|e| Error::Install(e.to_string()))?;
        let errors: Vec<String> = pool.install(|| {
            specs
                .par_iter()
                .filter_map(|s| match self.download(s) {
                    Ok(()) => {
                        bar.inc(1);
                        None
                    }
                    Err(e) => {
                        bar.inc(1);
                        Some(format!("{}: {e}", s.dest.display()))
                    }
                })
                .collect()
        });
        bar.finish_and_clear();
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
