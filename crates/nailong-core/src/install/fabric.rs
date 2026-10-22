use crate::blob;
use crate::cancel;
use crate::error::{Error, Result};
use crate::http::Http;
use crate::install::vanilla;
use crate::manifest::{Context, VersionJson, library_specs_for};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink};

pub fn profile_id(mc: &str, loader: &str) -> String {
    format!("fabric-loader-{loader}-{mc}")
}

pub fn install_with(
    root: &Root,
    mc: &str,
    loader: &str,
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    cancel::check()?;
    root.ensure()?;
    (sink)(Progress::stage("检查原版资源"));
    vanilla::install_with(root, mc, mirror, sink)?;

    (sink)(Progress::stage("获取 Fabric 配置"));
    let http = Http::new(mirror)?;
    let url = format!("https://meta.fabricmc.net/v2/versions/loader/{mc}/{loader}/profile/json");
    let value = http.get_json(&url)?;
    let profile: VersionJson = serde_json::from_value(value)
        .map_err(|e| Error::Install(format!("Fabric 配置解析失败: {e}")))?;
    let id = if profile.id.is_empty() {
        profile_id(mc, loader)
    } else {
        profile.id.clone()
    };
    let path = root.version_json(&id);
    write_atomic(&path, &serde_json::to_vec_pretty(&profile).unwrap())?;

    let ctx = Context::current();
    let specs = library_specs_for(root, &profile.libraries, &ctx);
    (sink)(Progress::stage("下载 Fabric 运行库"));
    blob::download_specs(root, &http, specs, "下载 Fabric 运行库", sink)?;

    (sink)(Progress {
        stage: "安装完成".into(),
        files_done: 1,
        files_total: 1,
        bytes_done: 1,
        bytes_total: 1,
        indeterminate: false,
    });
    println!("✓ Fabric {loader}（{mc}）安装完成");
    Ok(id)
}
