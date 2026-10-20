use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::install::vanilla;
use crate::java;
use crate::paths::Root;
use crate::progress::{Progress, Sink, noop};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn installer_url(version: &str) -> (String, String) {
    // 1.20.1 的 NeoForge 走 net.neoforged:forge 的旧坐标
    if version.starts_with("1.20.1-") {
        let base = "https://maven.neoforged.net/releases/net/neoforged/forge";
        let file = format!("forge-{version}-installer.jar");
        (format!("{base}/{version}/{file}"), file)
    } else {
        let base = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
        let file = format!("neoforge-{version}-installer.jar");
        (format!("{base}/{version}/{file}"), file)
    }
}

pub fn install(root: &Root, mc: &str, version: &str, mirror: &str) -> Result<()> {
    install_with(root, mc, version, mirror, &noop())
}

pub fn install_with(root: &Root, mc: &str, version: &str, mirror: &str, sink: &Sink) -> Result<()> {
    root.ensure()?;

    // Always run the vanilla pass: it verifies and completes missing resources.
    // This is what makes "继续安装" repair an interrupted download.
    (sink)(Progress::stage("检查原版资源"));
    println!("→ 检查 / 补全原版 {mc}");
    vanilla::install_with(root, mc, mirror, sink)?;

    // Download the official installer once.
    let (url, file) = installer_url(version);
    let dest = root.cache().join("installers").join(&file);
    let http = Http::new(mirror)?;
    (sink)(Progress::stage("下载 NeoForge 安装器"));
    println!("→ 获取 NeoForge {version} 安装器…");
    http.download_with(
        FileSpec {
            url,
            dest: dest.clone(),
            sha1: None,
            size: None,
            verify_existing: false,
        },
        "下载 NeoForge 安装器",
        sink,
    )?;

    // Run the official installer (one-time; the launcher stays native).
    let javas = java::find_all();
    let java = javas
        .iter()
        .find(|j| j.major == 21)
        .or_else(|| javas.iter().find(|j| j.major >= 21))
        .or_else(|| javas.first())
        .ok_or(Error::JavaNotFound(21))?;
    (sink)(Progress::stage("运行 NeoForge 安装器"));
    println!("→ 使用 {} 运行 NeoForge 安装器…", java.label());

    let profiles = root.dir().join("launcher_profiles.json");
    if !profiles.exists() {
        let _ = std::fs::write(&profiles, br#"{"profiles":{},"settings":{},"version":3}"#);
    }

    let log_path = root
        .cache()
        .join("logs")
        .join(format!("neoforge-{version}-install.log"));
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
    }
    let log = std::fs::File::create(&log_path).map_err(|e| crate::error::io(&log_path, e))?;
    let log_err = log
        .try_clone()
        .map_err(|e| crate::error::io(&log_path, e))?;

    let status = Command::new(&java.path)
        .arg("-jar")
        .arg(&dest)
        .arg("--installClient")
        .arg(root.dir())
        .current_dir(root.dir())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .status()
        .map_err(|e| Error::Install(format!("无法启动 NeoForge 安装器: {e}")))?;

    if !status.success() {
        return Err(Error::Install(format!(
            "NeoForge 安装器退出码 {status}；日志: {}",
            log_path.display()
        )));
    }

    let profile_id = format!("neoforge-{version}");
    let profile = root.version_json(&profile_id);
    if !profile.exists() {
        return Err(Error::Install(format!(
            "安装器结束但找不到版本文件: {}（日志: {}）",
            profile.display(),
            log_path.display()
        )));
    }
    (sink)(Progress {
        stage: "安装完成".into(),
        files_done: 1,
        files_total: 1,
        bytes_done: 1,
        bytes_total: 1,
        indeterminate: false,
    });
    println!("✓ NeoForge {version} 安装完成（profile: {profile_id}）");
    Ok(())
}

pub fn tail_log(path: &std::path::Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[allow(dead_code)]
fn write_stdout(msg: &str) {
    let _ = writeln!(std::io::stdout(), "{msg}");
}
