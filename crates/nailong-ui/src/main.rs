#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
//! Cairn 的图形界面。
//!
//! 结构和界面一样分层，一个文件管一件事：
//!
//! - `app`：全部状态和每一帧的流程；`msg`：后台结果回到界面的唯一入口。
//! - `pages/`：四个一级页面（启动台、版本、内容、存档）和进度页。
//! - `dialogs/`：盖在页面上的弹窗（设置、账户、启动参数、日志、恢复点、安装确认）。
//! - `widgets`：通用组件；`theme`、`motion`、`rings`：配色、动效和状态圆环。
//! - `model`：数据类型；`play`：开始游戏；`activity`：后台任务的进度；`fonts`：字体和图标。
//!
//! 每个模块开头 `use crate::prelude::*;`，页面只拼装组件、改自己的状态。

mod activity;
mod app;
mod dialogs;
mod fonts;
mod model;
mod motion;
mod msg;
mod pages;
mod play;
mod prelude;
mod rings;
#[cfg(test)]
mod tests;
mod theme;
mod widgets;

use prelude::*;

fn main() -> eframe::Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(|p| Root::new(PathBuf::from(p)))
        .unwrap_or_else(|| Root::new(Root::default_root()));
    #[cfg(debug_assertions)]
    let size = std::env::var("NAILONG_DEBUG_SIZE")
        .ok()
        .and_then(|value| {
            let (w, h) = value.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1120.0, 720.0]);
    #[cfg(not(debug_assertions))]
    let size = [1120.0, 720.0];
    let options = eframe::NativeOptions {
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([900.0, 620.0])
            .with_title("Cairn")
            .with_app_id("cairn")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "cairn",
        options,
        Box::new(move |cc| {
            t::install(&cc.egui_ctx);
            install_fonts(&cc.egui_ctx);
            root.migrate_legacy_cache();
            let app = App::new(root);
            app.prefs.theme.apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
