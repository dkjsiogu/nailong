use crate::error::Result;
use crate::install::{processors, slim, vanilla};
use crate::paths::Root;
use crate::progress::{Progress, Sink};

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
    // 7–9 MB installer, but a client install needs only ~3 MB of it.  Read
    // those entries directly and keep a small cache instead of the package.
    let dest = slim::prepare(root, &url, &file, mirror, "Forge", sink)?;

    (sink)(Progress::stage("安装 Forge"));
    let id = processors::install_client(root, mc, &dest, mirror, "Forge", sink)?;
    println!("✓ Forge {version}（{mc}）安装完成");
    Ok(id)
}
