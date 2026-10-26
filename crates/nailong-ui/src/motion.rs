//! Block-style interaction primitives: hard edges, bevels and pixel shadows.
//!
//! There are no rounded corners, blurry glass or gradients on interactive
//! elements. Depth comes from four one-pixel-ish tones (fill, top light,
//! bottom dark, black outline), the same language Minecraft's GUI uses.

use eframe::egui;
use egui::epaint::{Mesh, Shadow, Vertex, WHITE_UV};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::backdrop::{Backdrop, Scene};
use crate::{ACCENT, ACCENT_HI, ACCENT_LO, DIM, INK};

const ON_ACCENT: egui::Color32 = egui::Color32::from_rgb(7, 16, 10);
const BLACK: egui::Color32 = egui::Color32::from_rgb(3, 4, 5);

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| egui::lerp(x as f32..=y as f32, t).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
        ch(a.r(), b.r()),
        ch(a.g(), b.g()),
        ch(a.b(), b.b()),
        ch(a.a(), b.a()),
    )
}

pub fn gradient_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    top: egui::Color32,
    bottom: egui::Color32,
) {
    let mut mesh = Mesh::default();
    mesh.vertices.push(Vertex {
        pos: rect.left_top(),
        uv: WHITE_UV,
        color: top,
    });
    mesh.vertices.push(Vertex {
        pos: rect.right_top(),
        uv: WHITE_UV,
        color: top,
    });
    mesh.vertices.push(Vertex {
        pos: rect.right_bottom(),
        uv: WHITE_UV,
        color: bottom,
    });
    mesh.vertices.push(Vertex {
        pos: rect.left_bottom(),
        uv: WHITE_UV,
        color: bottom,
    });
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Kept for the few places that still want a two-tone plate. Square by
/// definition now; the radius argument is accepted for call-site compat.
pub fn gradient_rrect(
    painter: &egui::Painter,
    rect: egui::Rect,
    _radius: f32,
    top: egui::Color32,
    bottom: egui::Color32,
) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    gradient_rect(painter, rect, top, bottom);
}

/// Hard offset shadow: a dark block sitting three pixels down-right.
pub fn card_shadow() -> Shadow {
    Shadow {
        offset: [3, 3],
        blur: 0,
        spread: 0,
        color: egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190),
    }
}

/// No bloom in the deepslate language. Kept as a no-op so old call sites
/// keep compiling while the visual language stays flat.
pub fn glow(
    _painter: &egui::Painter,
    _rect: egui::Rect,
    _radius: impl Into<egui::CornerRadius>,
    _color: egui::Color32,
    _blur: u8,
) {
}

static REDUCED: AtomicBool = AtomicBool::new(false);

pub fn set_reduced(value: bool) {
    REDUCED.store(value, Ordering::Relaxed);
}

pub fn reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

pub fn paint_ambient(ui: &egui::Ui, _time: f64, cache: &mut Option<Backdrop>, dim: f32) {
    let rect = ui.max_rect();
    if rect.width() < 32.0 || rect.height() < 32.0 {
        return;
    }
    let painter = ui.painter();
    if cache.is_none() {
        painter.rect_filled(rect, 0.0, crate::BG);
    }
    Backdrop::ensure(ui.ctx(), cache, rect.size(), Scene::Window);
    if let Some(backdrop) = cache {
        backdrop.paint(painter, rect);
    }
    let dim = dim.clamp(0.0, 0.9);
    if dim > 0.001 {
        painter.rect_filled(
            rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(5, 6, 7, (dim * 255.0) as u8),
        );
    }
    // One hard seam at the top edge, no gradient.
    painter.rect_filled(
        egui::Rect::from_min_size(rect.left_top(), egui::vec2(rect.width(), 2.0)),
        0.0,
        egui::Color32::from_rgb(4, 5, 6),
    );
}

/// A four-pixel gold/emerald rule used before headings.
pub fn accent_bar(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, BLACK);
    let inner = rect.shrink(1.0);
    let split = inner.height() * 0.55;
    painter.rect_filled(
        egui::Rect::from_min_size(inner.left_top(), egui::vec2(inner.width(), split)),
        0.0,
        ACCENT,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(inner.left(), inner.top() + split),
            egui::vec2(inner.width(), inner.height() - split),
        ),
        0.0,
        ACCENT_LO,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrimaryGlyph {
    None,
    Play,
    Download,
}

fn block_body(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill: egui::Color32,
    light: egui::Color32,
    dark: egui::Color32,
    outline: egui::Color32,
) {
    painter.rect_filled(rect, 0.0, outline);
    let inner = rect.shrink(2.0);
    if inner.width() < 2.0 || inner.height() < 2.0 {
        return;
    }
    painter.rect_filled(inner, 0.0, fill);
    // Two-pixel bevel: light on top/left, shadow on bottom/right.
    painter.rect_filled(
        egui::Rect::from_min_size(inner.left_top(), egui::vec2(inner.width(), 2.0)),
        0.0,
        light,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(inner.left_top(), egui::vec2(2.0, inner.height())),
        0.0,
        light,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(inner.left(), inner.bottom() - 2.0),
            egui::vec2(inner.width(), 2.0),
        ),
        0.0,
        dark,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(inner.right() - 2.0, inner.top()),
            egui::vec2(2.0, inner.height()),
        ),
        0.0,
        dark,
    );
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

pub fn primary_sized_glyph(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    _font_size: f32,
    glyph: PrimaryGlyph,
) -> bool {
    let desired = egui::vec2(width.max(72.0), height.max(28.0));
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    let hover = if reduced() {
        if response.hovered() && enabled {
            1.0
        } else {
            0.0
        }
    } else {
        ui.ctx().animate_bool_with_time(
            response.id.with("hover"),
            response.hovered() && enabled,
            0.06,
        )
    };
    let pressed = enabled && response.is_pointer_button_down_on();
    let body = rect.translate(egui::vec2(0.0, if pressed { 2.0 } else { 0.0 }));
    if enabled {
        let fill = mix(ACCENT_LO, ACCENT, hover);
        block_body(
            ui.painter(),
            body,
            fill,
            mix(ACCENT_HI, egui::Color32::WHITE, hover * 0.15),
            egui::Color32::from_rgb(23, 92, 50),
            BLACK,
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        paint_button_content(ui.painter(), body, text, glyph, ON_ACCENT, BLACK);
    } else {
        block_body(
            ui.painter(),
            body,
            egui::Color32::from_rgb(52, 54, 55),
            egui::Color32::from_rgb(74, 76, 77),
            egui::Color32::from_rgb(28, 29, 30),
            BLACK,
        );
        paint_button_content(ui.painter(), body, text, glyph, DIM, BLACK);
    }
    response.clicked()
}

fn paint_button_content(
    painter: &egui::Painter,
    body: egui::Rect,
    text: &str,
    glyph: PrimaryGlyph,
    color: egui::Color32,
    shadow: egui::Color32,
) {
    let font = egui::FontId::new(12.0, crate::bold_family());
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let icon_w = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        12.0
    };
    let gap = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        8.0
    };
    let total = icon_w + gap + galley.size().x;
    let left = body.center().x - total * 0.5;
    if glyph != PrimaryGlyph::None {
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(left + icon_w * 0.5, body.center().y),
            egui::vec2(12.0, 12.0),
        );
        paint_primary_glyph(painter, icon_rect, glyph, color);
    }
    let pos = egui::pos2(left + icon_w + gap, body.center().y - galley.size().y * 0.5);
    let _ = shadow;
    painter.galley(pos, galley, color);
}

fn paint_primary_glyph(
    painter: &egui::Painter,
    rect: egui::Rect,
    glyph: PrimaryGlyph,
    color: egui::Color32,
) {
    let c = rect.center();
    match glyph {
        PrimaryGlyph::None => {}
        PrimaryGlyph::Play => {
            // Stair-stepped triangle, not an anti-aliased polygon.
            for row in 0..5 {
                let w = 2.0 + row as f32 * 2.0;
                let x = c.x - 4.0;
                let y = c.y - 5.0 + row as f32 * 2.0;
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, 2.0)),
                    0.0,
                    color,
                );
            }
        }
        PrimaryGlyph::Download => {
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(c.x - 1.0, c.y - 6.0), egui::vec2(2.0, 8.0)),
                0.0,
                color,
            );
            for row in 0..4 {
                let w = 8.0 - row as f32 * 2.0;
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(c.x - w * 0.5, c.y + 2.0 + row as f32),
                        egui::vec2(w, 1.0),
                    ),
                    0.0,
                    color,
                );
            }
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(c.x - 6.0, c.y + 6.0), egui::vec2(12.0, 2.0)),
                0.0,
                color,
            );
        }
    }
}

/// Stone secondary block with the same bevel language as the primary.
pub fn secondary_sized(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    _font_size: f32,
) -> egui::Response {
    let desired = egui::vec2(width.max(48.0), height.max(24.0));
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    let hover = if enabled {
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.06)
    } else {
        0.0
    };
    let pressed = enabled && response.is_pointer_button_down_on();
    let body = rect.translate(egui::vec2(0.0, if pressed { 2.0 } else { 0.0 }));
    let fill = mix(
        egui::Color32::from_rgb(42, 48, 55),
        egui::Color32::from_rgb(52, 64, 74),
        hover,
    );
    block_body(
        ui.painter(),
        body,
        fill,
        egui::Color32::from_rgb(74, 82, 92),
        egui::Color32::from_rgb(11, 13, 16),
        BLACK,
    );
    let color = if enabled { INK } else { DIM };
    let font = egui::FontId::new(12.0, crate::bold_family());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let pos = egui::pos2(
        body.center().x - galley.size().x * 0.5,
        body.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(pos, galley, color);
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Segmented XP bar: hard frame, flat green fill, dark pixel gaps.
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, indeterminate: bool, time: f64) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, BLACK);
    let track = rect.shrink(2.0);
    painter.rect_filled(track, 0.0, egui::Color32::from_rgb(10, 12, 14));
    let fill_rect = if indeterminate {
        let cycle = ((time as f32 * 0.35) % 1.0).clamp(0.0, 1.0);
        let seg = (track.width() * 0.28).max(60.0);
        let x = track.left() - seg + (track.width() + seg) * cycle;
        egui::Rect::from_min_max(
            egui::pos2(x.max(track.left()), track.top()),
            egui::pos2((x + seg).min(track.right()), track.bottom()),
        )
    } else {
        egui::Rect::from_min_max(
            track.left_top(),
            egui::pos2(
                track.left() + track.width() * fraction.clamp(0.0, 1.0),
                track.bottom(),
            ),
        )
    };
    if fill_rect.width() > 0.5 {
        painter.rect_filled(fill_rect, 0.0, ACCENT);
        painter.rect_filled(
            egui::Rect::from_min_size(fill_rect.left_top(), egui::vec2(fill_rect.width(), 2.0)),
            0.0,
            ACCENT_HI,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(fill_rect.left(), fill_rect.bottom() - 2.0),
                egui::vec2(fill_rect.width(), 2.0),
            ),
            0.0,
            ACCENT_LO,
        );
        // One-pixel notches every 12 px, like an XP bar.
        let mut x = fill_rect.left() + 12.0;
        while x < fill_rect.right() {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x, fill_rect.top()),
                    egui::vec2(1.0, fill_rect.height()),
                ),
                0.0,
                egui::Color32::from_rgba_unmultiplied(6, 30, 14, 120),
            );
            x += 12.0;
        }
    }
}
