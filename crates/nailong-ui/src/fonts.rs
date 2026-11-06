//! 字体：系统 CJK 字体、粗体和细体族；窗口图标。

use crate::prelude::*;

/// 中文正文与粗体分成两个字体族；`RichText::family(bold_family())` 才是真粗体。
pub(crate) fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// 页面标题用常规字重，层级靠字号表达。
pub(crate) fn display_family() -> egui::FontFamily {
    egui::FontFamily::Proportional
}

/// 启动台版本号这类大字用的细体。只有拉丁字符有细体，中文回退到常规字重。
pub(crate) fn light_family() -> egui::FontFamily {
    egui::FontFamily::Name("light".into())
}

pub(crate) fn install_fonts(ctx: &egui::Context) {
    // 中文用系统字体，不内置。Noto 的 ttc 里第 2 个字形集才是简体。
    const REGULAR: [(&str, u32); 6] = [
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
        (
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            2,
        ),
        (
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            0,
        ),
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),
        ("/System/Library/Fonts/PingFang.ttc", 0),
    ];
    const BOLD: [(&str, u32); 5] = [
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc", 2),
        ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Bold.ttc", 2),
        ("/usr/share/fonts/opentype/noto/NotoSansCJKsc-Bold.otf", 0),
        ("C:\\Windows\\Fonts\\msyhbd.ttc", 0),
    ];
    let mut fonts = egui::FontDefinitions::default();
    let mut load = |key: &str, candidates: &[(&str, u32)]| -> bool {
        for (path, index) in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = *index;
                fonts.font_data.insert(key.to_owned(), data.into());
                return true;
            }
        }
        false
    };
    let has_regular = load("cjk", &REGULAR);
    let has_bold = load("cjk-bold", &BOLD);
    let defaults = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut normal = Vec::new();
    if has_regular {
        normal.push("cjk".to_owned());
    }
    normal.extend(defaults);
    let mut bold = normal.clone();
    if has_bold {
        bold.insert(0, "cjk-bold".to_owned());
    }
    // Ubuntu Light 随 egui 默认字体一起分发，不增加体积。
    let mut light = vec!["Ubuntu-Light".to_owned()];
    light.extend(normal.clone());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.insert(family, normal.clone());
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("bold".into()), bold);
    fonts
        .families
        .insert(egui::FontFamily::Name("light".into()), light);
    ctx.set_fonts(fonts);
}

/// 窗口和任务栏图标：和 exe、deb 用同一张图。
pub(crate) fn app_icon() -> egui::IconData {
    const PNG: &[u8] =
        include_bytes!("../../../packaging/linux/icons/io.github.dkjsiogu.cairn-256.png");
    match image::load_from_memory(PNG) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}
