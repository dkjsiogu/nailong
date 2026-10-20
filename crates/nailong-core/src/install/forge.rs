use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::install::vanilla;
use crate::java;
use crate::paths::Root;
use crate::progress::{Progress, Sink};
use std::process::{Command, Stdio};

pub fn profile_id(mc: &str, version: &str) -> String {
    format!("{mc}-forge-{version}")
}

pub fn installer_url(mc: &str, version: &str) -> (String, String) {
    let full = format!("{mc}-{version}");
    let base = "https://maven.minecraftforge.net/net/minecraftforge/forge";
    let file = format!("forge-{full}-installer.jar");
    (format!("{base}/{full}/{file}"), file)
}

pub fn install_with(
    root: &Root,
    mc: &str,
    version: &str,
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    root.ensure()?;
    (sink)(Progress::stage("检查原版资源"));
    vanilla::install_with(root, mc, mirror, sink)?;

    let (url, file) = installer_url(mc, version);
    let dest = root.cache().join("installers").join(&file);
    let http = Http::new(mirror)?;
    (sink)(Progress::stage("下载 Forge 安装器"));
    http.download_with(
        FileSpec {
            url,
            dest: dest.clone(),
            sha1: None,
            size: None,
            verify_existing: false,
        },
        "下载 Forge 安装器",
        sink,
    )?;

    let javas = java::find_all();
    let java = javas
        .iter()
        .find(|j| j.major == 21)
        .or_else(|| javas.iter().find(|j| j.major >= 17))
        .or_else(|| javas.first())
        .ok_or(Error::JavaNotFound(21))?;
    (sink)(Progress::stage("运行 Forge 安装器"));
    println!("→ 使用 {} 运行 Forge 安装器…", java.label());

    let profiles = root.dir().join("launcher_profiles.json");
    if !profiles.exists() {
        let _ = std::fs::write(&profiles, br#"{"profiles":{},"settings":{},"version":3}"#);
    }

    let log_path = root
        .cache()
        .join("logs")
        .join(format!("forge-{mc}-{version}-install.log"));
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
        .map_err(|e| Error::Install(format!("无法启动 Forge 安装器: {e}")))?;
    if !status.success() {
        return Err(Error::Install(format!(
            "Forge 安装器退出码 {status}；日志: {}",
            log_path.display()
        )));
    }

    let id = profile_id(mc, version);
    let profile = root.version_json(&id);
    if !profile.exists() {
        // Older / unusual installers may still use the forge-<mc>-<ver> name.
        let legacy = format!("forge-{mc}-{version}");
        if root.version_json(&legacy).exists() {
            (sink)(Progress {
                stage: "安装完成".into(),
                files_done: 1,
                files_total: 1,
                bytes_done: 1,
                bytes_total: 1,
                indeterminate: false,
            });
            println!("✓ Forge {version}（{mc}）安装完成");
            return Ok(legacy);
        }
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
    println!("✓ Forge {version}（{mc}）安装完成");
    Ok(id)
}
