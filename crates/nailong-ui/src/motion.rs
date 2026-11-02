//! 基础控件和动效工具。
//!
//! 过渡都很短，结束后不再请求重绘，所以空闲时 CPU 是 0。

use eframe::egui;
use egui::{Color32, Rect, Sense, Shape, Stroke, pos2, vec2};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::theme as t;

/// 控件和卡片的圆角。
pub const R: u8 = 4;

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| egui::lerp(x as f32..=y as f32, t).round() as u8;
    Color32::from_rgba_premultiplied(
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
        ch(a.a(), b.a()),
    )
}

/// 欠阻尼弹簧，只用在状态圆环上。运动中才请求下一帧。
pub fn spring(ctx: &egui::Context, id: egui::Id, target: f32, stiffness: f32, damping: f32) -> f32 {
    if reduced() {
        return target;
    }
    let dt = ctx.input(|input| input.stable_dt).min(1.0 / 20.0);
    let (mut x, mut v) = ctx
        .data(|data| data.get_temp::<(f32, f32)>(id))
        .unwrap_or((target, 0.0));
    let steps = 4;
    let h = dt / steps as f32;
    for _ in 0..steps {
        v += (stiffness * (target - x) - damping * v) * h;
        x += v * h;
    }
    if (x - target).abs() < 0.002 && v.abs() < 0.01 {
        x = target;
        v = 0.0;
    } else {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, (x, v)));
    x
}

static REDUCED: AtomicBool = AtomicBool::new(false);

pub fn set_reduced(value: bool) {
    REDUCED.store(value, Ordering::Relaxed);
}

pub fn reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

/// 悬停过渡的进度；减少动效时直接到位。
pub fn hover_amount(ui: &egui::Ui, id: egui::Id, hovered: bool) -> f32 {
    if reduced() {
        return if hovered { 1.0 } else { 0.0 };
    }
    ui.ctx().animate_bool_with_time(id, hovered, 0.12)
}

/// 键盘聚焦的外框。每帧统一画一次，所有控件都能用上。
pub fn focus_ring(ctx: &egui::Context) {
    let Some(id) = ctx.memory(|memory| memory.focused()) else {
        return;
    };
    let Some(response) = ctx.read_response(id) else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("nailong_focus_ring"),
    ));
    painter.rect_stroke(
        response.rect.expand(2.0),
        R + 2,
        Stroke::new(2.0, t::accent()),
        egui::StrokeKind::Outside,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrimaryGlyph {
    None,
    Play,
    Download,
}

pub fn primary_sized(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    font_size: f32,
) -> bool {
    primary_sized_glyph(
        ui,
        text,
        enabled,
        width,
        height,
        font_size,
        PrimaryGlyph::None,
    )
}

/// 主按钮：一屏最多一个，只变颜色，不位移。
pub fn primary_sized_glyph(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    _font_size: f32,
    glyph: PrimaryGlyph,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width.max(72.0), height.max(32.0)), sense);
    let hover = hover_amount(ui, response.id.with("hover"), enabled && response.hovered());
    let (fill, color) = if !enabled {
        (t::wash(), t::dim())
    } else if response.is_pointer_button_down_on() {
        (t::accent_press(), t::on_accent())
    } else {
        (mix(t::accent(), t::accent_hover(), hover), t::on_accent())
    };
    ui.painter().rect_filled(rect, R, fill);
    paint_label(ui.painter(), rect, text, glyph, color, 14.0, true);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn paint_label(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    glyph: PrimaryGlyph,
    color: Color32,
    font_px: f32,
    bold: bool,
) {
    let family = if bold {
        crate::bold_family()
    } else {
        egui::FontFamily::Proportional
    };
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::new(font_px, family), color);
    let icon_w = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        14.0
    };
    let gap = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        8.0
    };
    let left = rect.center().x - (icon_w + gap + galley.size().x) * 0.5;
    if glyph != PrimaryGlyph::None {
        paint_glyph(
            painter,
            pos2(left + icon_w * 0.5, rect.center().y),
            glyph,
            color,
        );
    }
    painter.galley(
        pos2(left + icon_w + gap, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
}

fn paint_glyph(painter: &egui::Painter, c: egui::Pos2, glyph: PrimaryGlyph, color: Color32) {
    match glyph {
        PrimaryGlyph::None => {}
        PrimaryGlyph::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    c + vec2(-4.0, -6.0),
                    c + vec2(6.0, 0.0),
                    c + vec2(-4.0, 6.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        PrimaryGlyph::Download => {
            let stroke = Stroke::new(1.6, color);
            painter.line_segment([c + vec2(0.0, -6.0), c + vec2(0.0, 2.0)], stroke);
            painter.add(Shape::line(
                vec![
                    c + vec2(-4.0, -2.0),
                    c + vec2(0.0, 2.5),
                    c + vec2(4.0, -2.0),
                ],
                stroke,
            ));
            painter.line_segment([c + vec2(-6.0, 6.0), c + vec2(6.0, 6.0)], stroke);
        }
    }
}

/// 次按钮：透明底加细线，悬停时填色。
pub fn secondary_sized(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    _font_size: f32,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width.max(48.0), height.max(32.0)), sense);
    let hover = hover_amount(ui, response.id.with("hover"), enabled && response.hovered());
    if hover > 0.0 {
        ui.painter().rect_filled(rect, R, t::tint(t::wash(), hover));
    }
    ui.painter().rect_stroke(
        rect,
        R,
        Stroke::new(1.0, mix(t::hairline(), t::control(), hover)),
        egui::StrokeKind::Inside,
    );
    let color = if enabled { t::ink() } else { t::dim() };
    paint_label(
        ui.painter(),
        rect,
        text,
        PrimaryGlyph::None,
        color,
        14.0,
        false,
    );
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// 高 4 的细进度条。
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, indeterminate: bool, time: f64) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2, t::hairline());
    let time = if reduced() { 0.0 } else { time as f32 };
    let (from, to) = if indeterminate {
        let seg = (rect.width() * 0.25).max(48.0);
        let x = rect.left() - seg + (rect.width() + seg) * (time * 0.4).rem_euclid(1.0);
        (x.max(rect.left()), (x + seg).min(rect.right()))
    } else {
        (
            rect.left(),
            rect.left() + rect.width() * fraction.clamp(0.0, 1.0),
        )
    };
    if to - from > 0.5 {
        painter.rect_filled(
            Rect::from_min_max(pos2(from, rect.top()), pos2(to, rect.bottom())),
            2,
            t::accent(),
        );
    }
}
