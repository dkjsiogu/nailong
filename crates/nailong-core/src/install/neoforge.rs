use crate::cancel;
use crate::error::Result;
use crate::install::{health, join_install, processors, reuse, slim, vanilla};
use crate::paths::Root;
use crate::progress::{Progress, Sink, noop};
use std::thread;

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
    cancel::check()?;
    root.ensure()?;

    let installed_id = format!("neoforge-{version}");
    if root.version_json(&installed_id).exists()
        && health::inspect(root, &installed_id).is_healthy()
    {
        println!("✓ NeoForge {version}（{mc}）已安装，跳过下载");
        (sink)(Progress {
            stage: "安装完成".into(),
            files_done: 1,
            files_total: 1,
            bytes_done: 1,
            bytes_total: 1,
            indeterminate: false,
        });
        return Ok(installed_id);
    }

    let (url, file) = installer_url(version);
    (sink)(Progress::stage("准备安装"));
    println!("→ 检查 / 补全原版 {mc}");
    let vj = vanilla::prepare_version(root, mc, mirror, sink)?;
    let assets = {
        let root = root.clone();
        let mc = mc.to_string();
        let mirror = mirror.to_string();
        let sink = sink.clone();
        thread::spawn(move || vanilla::install_assets_with(&root, &mc, &mirror, &sink))
    };
    let mut reuse_version = None;
    match slim::fetch_metadata_any(&url, mirror) {
        Ok((profile, version_text)) => {
            if profile.get("minecraft").and_then(|v| v.as_str()) == Some(mc)
                && let Some(target) = reuse::patched_target(&profile)
            {
                match reuse::import_patched(root, &target) {
                    Ok(Some(_)) => reuse_version = Some(version_text),
                    Ok(None) => {}
                    Err(e) => println!("! 检查本机 patched client 失败: {e}"),
                }
            }
        }
        Err(e) => println!("! 读取 NeoForge 元数据失败，回退完整安装: {e}"),
    }
    if let Some(version_text) = reuse_version {
        if let Err(e) = vanilla::install_client_libs(root, mc, mirror, &vj, sink) {
            let _ = join_install(assets, "游戏资源");
            return Err(e);
        }
        let id = match reuse::install_version_profile(root, &version_text, mc, mirror, sink) {
            Ok(id) => id,
            Err(e) => {
                let _ = join_install(assets, "游戏资源");
                return Err(e);
            }
        };
        let assets_result = join_install(assets, "游戏资源");
        assets_result?;
        (sink)(Progress {
            stage: "安装完成".into(),
            files_done: 1,
            files_total: 1,
            bytes_done: 1,
            bytes_total: 1,
            indeterminate: false,
        });
        println!("✓ NeoForge {version}（{mc}）安装完成（复用本机客户端，profile: {id}）");
        return Ok(id);
    }

    let slim_job = {
        let root = root.clone();
        let url = url.clone();
        let file = file.clone();
        let mirror = mirror.to_string();
        let sink = sink.clone();
        thread::spawn(move || slim::prepare(&root, &url, &file, &mirror, "NeoForge", &sink))
    };

    if let Err(e) = vanilla::install_client_libs(root, mc, mirror, &vj, sink) {
        let _ = join_install(assets, "游戏资源");
        return Err(e);
    }
    println!("→ 读取 NeoForge {version} 安装信息…");
    let dest = match join_install(slim_job, "NeoForge 安装数据") {
        Ok(path) => path,
        Err(e) => {
            let _ = join_install(assets, "游戏资源");
            return Err(e);
        }
    };

    (sink)(Progress::stage("安装 NeoForge"));
    let processed = processors::install_client(root, mc, &dest, mirror, "NeoForge", sink);
    let assets_result = join_install(assets, "游戏资源");
    let id = processed?;
    assets_result?;
    (sink)(Progress {
        stage: "安装完成".into(),
        files_done: 1,
        files_total: 1,
        bytes_done: 1,
        bytes_total: 1,
        indeterminate: false,
    });
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
