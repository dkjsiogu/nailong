use crate::error::Result;
use crate::http::{FileSpec, Http};
use crate::install::{processors, vanilla};
use crate::paths::Root;
use crate::progress::{Progress, Sink, noop};

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

pub fn install(root: &Root, mc: &str, version: &str, mirror: &str) -> Result<String> {
    install_with(root, mc, version, mirror, &noop())
}

pub fn install_with(
    root: &Root,
    mc: &str,
    version: &str,
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    root.ensure()?;

    // Always run the vanilla pass: it verifies and completes missing resources.
    // This is what makes "继续安装" repair an interrupted download.
    (sink)(Progress::stage("检查原版资源"));
    println!("→ 检查 / 补全原版 {mc}");
    vanilla::install_with(root, mc, mirror, sink)?;

    // The installer package is data only; its processors are executed below.
    let (url, file) = installer_url(version);
    let dest = root.cache().join("installers").join(&file);
    let http = Http::new(mirror)?;
    (sink)(Progress::stage("下载 NeoForge 安装包"));
    println!("→ 获取 NeoForge {version} 安装包…");
    http.download_with(
        FileSpec {
            url,
            dest: dest.clone(),
            sha1: None,
            size: None,
            verify_existing: false,
        },
        "下载 NeoForge 安装包",
        sink,
    )?;

    (sink)(Progress::stage("安装 NeoForge"));
    let id = processors::install_client(root, mc, &dest, mirror, "NeoForge", sink)?;
    println!("✓ NeoForge {version}（{mc}）安装完成（profile: {id}）");
    Ok(id)
}

pub fn tail_log(path: &std::path::Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}
