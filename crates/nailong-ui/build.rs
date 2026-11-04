//! Windows：把图标和版本信息写进 exe。资源管理器、任务栏和「属性 › 详细信息」都读这里。
//! 其他平台什么都不做；Linux 的图标在 deb 里。

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/cairn.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../packaging/windows/cairn.ico")
        .set("ProductName", "Cairn")
        .set("FileDescription", "Cairn · Minecraft 启动器")
        .set("CompanyName", "dkjsiogu")
        .set("LegalCopyright", "© dkjsiogu · GPL-3.0-or-later")
        .set("OriginalFilename", "cairn.exe")
        .set("InternalName", "cairn")
        .set("ProductVersion", version)
        .set("FileVersion", version)
        .set_language(0x0804);
    if let Err(e) = res.compile() {
        // 没有资源编译器时照样出 exe，只是没有图标；CI 里会检查。
        println!("cargo:warning=没能写入 Windows 图标和版本信息：{e}");
    }
}
