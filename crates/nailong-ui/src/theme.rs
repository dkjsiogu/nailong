//! 雾蓝主题：深浅两套颜色共用同一组 token，色值见 `docs/design/18-ui-fog.md`。
//!
//! 除了这个文件，界面代码不写死颜色。

use eframe::egui;
use egui::Color32;
use std::sync::atomic::{AtomicBool, Ordering};

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

#[derive(Clone, Copy)]
struct Palette {
    bg: Color32,
    surface: Color32,
    wash: Color32,
    hairline: Color32,
    control: Color32,
    ink: Color32,
    sub: Color32,
    dim: Color32,
    dot: Color32,
    accent: Color32,
    accent_hover: Color32,
    accent_press: Color32,
    on_accent: Color32,
    ok: Color32,
    warn: Color32,
    danger: Color32,
}

const DARK: Palette = Palette {
    bg: rgb(0x1B, 0x1D, 0x22),
    surface: rgb(0x22, 0x25, 0x2B),
    wash: rgb(0x27, 0x2A, 0x31),
    hairline: rgb(0x2D, 0x30, 0x37),
    control: rgb(0x66, 0x6B, 0x76),
    ink: rgb(0xDF, 0xE2, 0xE8),
    sub: rgb(0xA0, 0xA5, 0xB0),
    dim: rgb(0x88, 0x8D, 0x98),
    dot: rgb(0x7C, 0x81, 0x8C),
    accent: rgb(0x84, 0x96, 0xC2),
    accent_hover: rgb(0x93, 0xA4, 0xCD),
    accent_press: rgb(0x75, 0x87, 0xB3),
    on_accent: rgb(0x14, 0x16, 0x1B),
    ok: rgb(0x88, 0xA8, 0x96),
    warn: rgb(0xBF, 0xA1, 0x73),
    danger: rgb(0xC9, 0x8A, 0x82),
};

const LIGHT: Palette = Palette {
    bg: rgb(0xF1, 0xF1, 0xEE),
    surface: rgb(0xFA, 0xFA, 0xF7),
    wash: rgb(0xE4, 0xE5, 0xE4),
    hairline: rgb(0xDB, 0xDB, 0xD6),
    control: rgb(0x85, 0x88, 0x8F),
    ink: rgb(0x2B, 0x2D, 0x32),
    sub: rgb(0x5C, 0x5F, 0x66),
    dim: rgb(0x6B, 0x6E, 0x74),
    dot: rgb(0x85, 0x88, 0x8F),
    accent: rgb(0x52, 0x64, 0x8F),
    accent_hover: rgb(0x5E, 0x70, 0x9B),
    accent_press: rgb(0x47, 0x58, 0x80),
    on_accent: rgb(0xF6, 0xF6, 0xF3),
    ok: rgb(0x51, 0x76, 0x62),
    warn: rgb(0x85, 0x69, 0x36),
    danger: rgb(0xA5, 0x54, 0x4C),
};

static IS_LIGHT: AtomicBool = AtomicBool::new(false);

fn palette() -> Palette {
    if IS_LIGHT.load(Ordering::Relaxed) {
        LIGHT
    } else {
        DARK
    }
}

macro_rules! tokens {
    ($($name:ident),* $(,)?) => {
        $(pub fn $name() -> Color32 {
            palette().$name
        })*
    };
}

tokens!(
    bg,
    surface,
    wash,
    hairline,
    control,
    ink,
    sub,
    dim,
    dot,
    accent,
    accent_hover,
    accent_press,
    on_accent,
    ok,
    warn,
    danger,
);

/// 把颜色调成半透明，用于状态提示的底色和边框。
pub fn tint(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}

/// 弹窗后面的遮罩。
pub fn veil(alpha: f32) -> Color32 {
    let alpha = alpha.clamp(0.0, 1.0);
    if IS_LIGHT.load(Ordering::Relaxed) {
        Color32::from_black_alpha((alpha * 90.0) as u8)
    } else {
        Color32::from_black_alpha((alpha * 150.0) as u8)
    }
}

/// 外观设置，存在 `ui.json` 里。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preference {
    System,
    Dark,
    Light,
}

impl Preference {
    pub const ALL: [Preference; 3] = [Preference::System, Preference::Dark, Preference::Light];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Dark => "深色",
            Self::Light => "浅色",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "dark" => Self::Dark,
            "light" => Self::Light,
            _ => Self::System,
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        ctx.set_theme(match self {
            Self::System => egui::ThemePreference::System,
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
        });
        sync(ctx);
    }
}

/// 每帧开始时调用：系统外观可能在运行中变化。
pub fn sync(ctx: &egui::Context) {
    IS_LIGHT.store(ctx.theme() == egui::Theme::Light, Ordering::Relaxed);
}

fn visuals(p: Palette, dark: bool) -> egui::Visuals {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let radius = egui::CornerRadius::same(4);
    let shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 70 } else { 28 }),
    };
    visuals.override_text_color = Some(p.ink);
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.surface;
    visuals.extreme_bg_color = p.surface;
    visuals.window_stroke = egui::Stroke::new(1.0, p.hairline);
    visuals.window_corner_radius = egui::CornerRadius::same(6);
    visuals.menu_corner_radius = radius;
    visuals.popup_shadow = shadow;
    visuals.window_shadow = shadow;
    visuals.faint_bg_color = p.wash;
    visuals.code_bg_color = p.wash;
    visuals.selection.bg_fill = p.accent.gamma_multiply(0.35);
    visuals.selection.stroke = egui::Stroke::new(1.0, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.slider_trailing_fill = true;
    visuals.text_cursor.stroke = egui::Stroke::new(2.0, p.accent);
    let widget = |fill: Color32, stroke: Color32| egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: egui::Stroke::new(1.0, stroke),
        corner_radius: radius,
        fg_stroke: egui::Stroke::new(1.0, p.ink),
        expansion: 0.0,
    };
    visuals.widgets.noninteractive = widget(Color32::TRANSPARENT, p.hairline);
    visuals.widgets.inactive = widget(Color32::TRANSPARENT, p.hairline);
    visuals.widgets.hovered = widget(p.wash, p.control);
    visuals.widgets.active = widget(p.wash, p.control);
    visuals.widgets.open = widget(p.wash, p.control);
    visuals
}

/// 启动时注册两套样式；之后只需切换 `ThemePreference`。
pub fn install(ctx: &egui::Context) {
    for (theme, palette, dark) in [
        (egui::Theme::Dark, DARK, true),
        (egui::Theme::Light, LIGHT, false),
    ] {
        let mut style = (*ctx.style_of(theme)).clone();
        style.visuals = visuals(palette, dark);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.animation_time = 0.12;
        style.text_styles = [
            (
                egui::TextStyle::Heading,
                egui::FontId::new(24.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Body,
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Monospace,
                egui::FontId::new(13.0, egui::FontFamily::Monospace),
            ),
            (
                egui::TextStyle::Button,
                egui::FontId::new(14.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Small,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
            ),
        ]
        .into();
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        ctx.set_style_of(theme, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let channel = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn both_palettes_meet_the_spec_contrast() {
        for p in [DARK, LIGHT] {
            for text in [p.ink, p.sub, p.dim, p.accent, p.ok, p.warn, p.danger] {
                assert!(contrast(text, p.bg) >= 4.5, "{text:?} on bg");
            }
            for text in [p.ink, p.sub, p.dim] {
                assert!(contrast(text, p.surface) >= 4.5, "{text:?} on surface");
            }
            assert!(contrast(p.on_accent, p.accent) >= 4.5);
            assert!(contrast(p.control, p.bg) >= 3.0);
        }
    }

    #[test]
    fn preference_round_trips_through_its_key() {
        for pref in Preference::ALL {
            assert_eq!(Preference::from_key(pref.key()), pref);
        }
        assert_eq!(Preference::from_key("anything"), Preference::System);
    }
}
