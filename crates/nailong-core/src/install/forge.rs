use crate::error::Result;
use crate::install::{health, join_install, processors, slim, vanilla};
use crate::paths::Root;
use crate::progress::{Progress, Sink};
use std::thread;

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
    let id = profile_id(mc, version);
    if root.version_json(&id).exists() && health::inspect(root, &id).is_healthy() {
        println!("✓ Forge {version}（{mc}）已安装，跳过下载");
        (sink)(Progress {
            stage: "安装完成".into(),
            files_done: 1,
            files_total: 1,
            bytes_done: 1,
            bytes_total: 1,
            indeterminate: false,
        });
        return Ok(id);
    }

    let (url, file) = installer_url(mc, version);
    (sink)(Progress::stage("准备安装"));
    let vj = vanilla::prepare_version(root, mc, mirror, sink)?;

    // Assets do not depend on loader processors.  Start them first, then run
    // client/libraries, slim metadata and processors while assets continue.
    let assets = {
        let root = root.clone();
        let mc = mc.to_string();
        let mirror = mirror.to_string();
        let sink = sink.clone();
        thread::spawn(move || vanilla::install_assets_with(&root, &mc, &mirror, &sink))
    };
    let slim_job = {
        let root = root.clone();
        let url = url.clone();
        let file = file.clone();
        let mirror = mirror.to_string();
        let sink = sink.clone();
        thread::spawn(move || slim::prepare(&root, &url, &file, &mirror, "Forge", &sink))
    };

    if let Err(e) = vanilla::install_client_libs(root, mc, mirror, &vj, sink) {
        let _ = join_install(assets, "游戏资源");
        return Err(e);
    }
    let dest = match join_install(slim_job, "Forge 安装数据") {
        Ok(path) => path,
        Err(e) => {
            let _ = join_install(assets, "游戏资源");
            return Err(e);
        }
    };

    (sink)(Progress::stage("安装 Forge"));
    let processed = processors::install_client(root, mc, &dest, mirror, "Forge", sink);
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
    println!("✓ Forge {version}（{mc}）安装完成");
    Ok(id)
}
