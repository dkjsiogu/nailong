//! Light, depth and motion helpers shared by every screen.
//!
//! The launcher is still immediate-mode egui, so "animation" here means:
//! - values read back through `Context::animate_bool/value`
//! - explicit enter timers for page/modal transitions
//! - hand-painted gradients, glows and ambient embers
//!
//! Everything is deliberately low-contrast: the goal is depth, not neon.

use eframe::egui;
use egui::epaint::{Mesh, Shadow, Vertex, WHITE_UV};

use crate::{ACCENT_HI, ACCENT_LO, INK, LINE, MUTED, SURFACE2};

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

/// Vertical gradient over the whole content area (square corners, two triangles).
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

/// Horizontal gradient, used for thin accent rules.
pub fn gradient_rrect(
    painter: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    top: egui::Color32,
    bottom: egui::Color32,
) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let r = radius
        .max(0.0)
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5);
    let mut points = Vec::with_capacity(32);
    // TL -> TR -> BR -> BL, clockwise.
    let corners = [
        (rect.left() + r, rect.top() + r, std::f32::consts::PI),
        (rect.right() - r, rect.top() + r, std::f32::consts::PI * 1.5),
        (rect.right() - r, rect.bottom() - r, 0.0),
        (
            rect.left() + r,
            rect.bottom() - r,
            std::f32::consts::PI * 0.5,
        ),
    ];
    let steps = 5;
    for (cx, cy, start) in corners {
        for step in 0..steps {
            let a = start + std::f32::consts::FRAC_PI_2 * (step as f32 / steps as f32);
            points.push(egui::pos2(cx + a.cos() * r, cy + a.sin() * r));
        }
    }
    let color_at = |p: egui::Pos2| {
        let t = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        mix(top, bottom, t)
    };
    let mut mesh = Mesh::default();
    let center = rect.center();
    mesh.vertices.push(Vertex {
        pos: center,
        uv: WHITE_UV,
        color: color_at(center),
    });
    for p in &points {
        mesh.vertices.push(Vertex {
            pos: *p,
            uv: WHITE_UV,
            color: color_at(*p),
        });
    }
    let n = points.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Soft, colourless depth under cards. Cheap enough to use on every card.
pub fn card_shadow() -> Shadow {
    Shadow {
        offset: [0, 16],
        blur: 38,
        spread: 0,
        color: egui::Color32::from_rgba_unmultiplied(120, 90, 55, 46),
    }
}

pub fn glow(
    painter: &egui::Painter,
    rect: egui::Rect,
    radius: impl Into<egui::CornerRadius>,
    color: egui::Color32,
    blur: u8,
) {
    if color.a() == 0 {
        return;
    }
    painter.add(
        Shadow {
            offset: [0, 0],
            blur,
            spread: 0,
            color,
        }
        .as_shape(rect, radius),
    );
}

fn radial_glow(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: egui::Color32) {
    if radius <= 1.0 || color.a() == 0 {
        return;
    }
    let edge = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 0);
    let segments = 72usize;
    let rings = 6usize;
    let mut mesh = Mesh::default();
    // Center + `rings` concentric rings; alpha falls off quadratically.
    mesh.vertices.push(Vertex {
        pos: center,
        uv: WHITE_UV,
        color,
    });
    for ring in 1..=rings {
        let r = radius * ring as f32 / rings as f32;
        let falloff = 1.0 - (ring as f32 / rings as f32);
        let c = egui::Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (color.a() as f32 * falloff * falloff) as u8,
        );
        for i in 0..segments {
            let a = i as f32 / segments as f32 * std::f32::consts::TAU;
            mesh.vertices.push(Vertex {
                pos: center + egui::vec2(a.cos(), a.sin()) * r,
                uv: WHITE_UV,
                color: if ring == rings { edge } else { c },
            });
        }
    }
    for i in 0..segments {
        let next = (i + 1) % segments;
        mesh.add_triangle(0, 1 + i as u32, 1 + next as u32);
    }
    for ring in 0..(rings - 1) {
        let base = 1 + ring * segments;
        let next_base = base + segments;
        for i in 0..segments {
            let next = (i + 1) % segments;
            let a = (base + i) as u32;
            let b = (base + next) as u32;
            let c = (next_base + i) as u32;
            let d = (next_base + next) as u32;
            mesh.add_triangle(a, c, b);
            mesh.add_triangle(b, c, d);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}
/// Painted before page content; fixed while the page scrolls.
pub fn paint_ambient(ui: &egui::Ui, time: f64) {
    let rect = ui.max_rect();
    if rect.width() < 32.0 || rect.height() < 32.0 {
        return;
    }
    let painter = ui.painter();
    gradient_rect(
        painter,
        rect,
        egui::Color32::from_rgb(255, 250, 242),
        egui::Color32::from_rgb(255, 243, 229),
    );
    // A very quiet dot grid gives the large empty areas some texture.
    let step = 44.0;
    let dot = egui::Color32::from_rgba_unmultiplied(203, 178, 146, 42);
    let mut y = rect.top() + 22.0;
    while y < rect.bottom() {
        let mut x = rect.left() + 30.0;
        while x < rect.right() {
            painter.circle_filled(egui::pos2(x, y), 0.65, dot);
            x += step;
        }
        y += step;
    }
    let breathe = ((time * 0.45).sin() as f32 * 0.5 + 0.5).clamp(0.0, 1.0);
    radial_glow(
        painter,
        egui::pos2(
            rect.left() + rect.width() * 0.20,
            rect.top() + rect.height() * 0.20,
        ),
        rect.width() * 0.68,
        egui::Color32::from_rgba_unmultiplied(255, 178, 72, (10.0 + 8.0 * breathe) as u8),
    );
    radial_glow(
        painter,
        egui::pos2(
            rect.right() - rect.width() * 0.08,
            rect.bottom() + rect.height() * 0.06,
        ),
        rect.width() * 0.55,
        egui::Color32::from_rgba_unmultiplied(255, 168, 74, 7),
    );

    if let Some(pointer) = ui.ctx().pointer_hover_pos()
        && rect.contains(pointer)
    {
        radial_glow(
            painter,
            pointer,
            300.0,
            egui::Color32::from_rgba_unmultiplied(255, 170, 55, 14),
        );
    }

    // A handful of almost-invisible embers drifting upwards. They are painted
    // behind cards, so only the gutters/empty space show them.
    let t = time as f32;
    for i in 0..12 {
        let seed = i as f32 * 0.618_034;
        let speed = 5.0 + (i % 4) as f32 * 2.2;
        let traveled = (t * speed + seed * 97.0) % (rect.height() + 90.0);
        let y = rect.bottom() + 30.0 - traveled;
        let x = rect.left()
            + ((seed * 91.7).sin() * 0.5 + 0.5) * rect.width()
            + (t * 0.35 + seed * std::f32::consts::TAU).sin() * 14.0;
        let alpha = (7.0 + (i % 3) as f32 * 6.0) as u8;
        let radius = 0.8 + (i % 3) as f32 * 0.45;
        painter.circle_filled(
            egui::pos2(x, y),
            radius,
            egui::Color32::from_rgba_unmultiplied(226, 138, 40, alpha),
        );
    }

    // A thin warm line at the very bottom closes the page softly.
    let line = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.bottom() - 1.0),
        egui::vec2(rect.width(), 1.0),
    );
    painter.add(egui::Shape::mesh({
        let mut mesh = Mesh::default();
        let c0 = egui::Color32::from_rgba_unmultiplied(245, 158, 11, 38);
        let c1 = egui::Color32::from_rgba_unmultiplied(245, 158, 11, 0);
        mesh.vertices.push(Vertex {
            pos: line.left_top(),
            uv: WHITE_UV,
            color: c0,
        });
        mesh.vertices.push(Vertex {
            pos: line.right_top(),
            uv: WHITE_UV,
            color: c1,
        });
        mesh.vertices.push(Vertex {
            pos: line.right_bottom(),
            uv: WHITE_UV,
            color: c1,
        });
        mesh.vertices.push(Vertex {
            pos: line.left_bottom(),
            uv: WHITE_UV,
            color: c0,
        });
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        mesh
    }));
}

/// Small vertical accent rule used before page titles.
pub fn accent_bar(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, height), egui::Sense::hover());
    gradient_rrect(ui.painter(), rect, 1.5, ACCENT_HI, ACCENT_LO);
    glow(
        ui.painter(),
        rect,
        2,
        egui::Color32::from_rgba_unmultiplied(245, 158, 11, 65),
        10,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrimaryGlyph {
    None,
    Play,
    Download,
}

/// Primary action: matte amber, a fine top edge and a warm contact shadow.
///
/// The earlier version relied on a bright vertical gradient plus a white gloss
/// line, which reads as a generic "web button". This one keeps the gradient
/// narrow, adds a real border/contact shadow and only lifts on hover.
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
    font_size: f32,
    glyph: PrimaryGlyph,
) -> bool {
    let desired = egui::vec2(width.max(72.0), height.max(28.0));
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() && enabled,
        0.16,
    );
    let pressed = enabled && response.is_pointer_button_down_on();
    let radius = (height * 0.24).clamp(8.0, 10.0);
    let painter = ui.painter();
    let body = if enabled {
        rect.translate(egui::vec2(0.0, -hover + if pressed { 1.0 } else { 0.0 }))
    } else {
        rect
    };
    if enabled {
        // Two shadows: a tight contact shadow keeps the button grounded, the
        // wider warm one gives it depth without the old orange halo.
        painter.add(
            Shadow {
                offset: [0, if pressed { 2 } else { 7 + (hover * 2.0) as i8 }],
                blur: if pressed { 8 } else { 20 + (hover * 6.0) as u8 },
                spread: 0,
                color: egui::Color32::from_rgba_unmultiplied(
                    132,
                    82,
                    24,
                    (20.0 + 8.0 * hover) as u8,
                ),
            }
            .as_shape(body, radius),
        );
        painter.add(
            Shadow {
                offset: [0, 1],
                blur: 2,
                spread: 0,
                color: egui::Color32::from_rgba_unmultiplied(92, 52, 12, 42),
            }
            .as_shape(body, radius),
        );

        let top = mix(
            egui::Color32::from_rgb(247, 178, 58),
            egui::Color32::WHITE,
            hover * 0.04,
        );
        let bottom = mix(
            egui::Color32::from_rgb(239, 155, 26),
            egui::Color32::from_rgb(247, 178, 58),
            hover * 0.05,
        );
        let top = if pressed { mix(top, bottom, 0.32) } else { top };
        gradient_rrect(painter, body, radius, top, bottom);

        // Just a hairline of inner light and a darker bottom seam. No wide
        // glossy band: that was what made the old button feel like plastic.
        let inner = body.shrink(1.0);
        let inset = radius + 1.0;
        painter.line_segment(
            [
                egui::pos2(inner.left() + inset, inner.top() + 0.5),
                egui::pos2(inner.right() - inset, inner.top() + 0.5),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_white_alpha(if pressed { 36 } else { 62 }),
            ),
        );
        let lower = egui::Rect::from_min_max(
            egui::pos2(inner.left() + inset, inner.bottom() - 2.0),
            egui::pos2(inner.right() - inset, inner.bottom()),
        );
        gradient_rect(
            painter,
            lower,
            egui::Color32::TRANSPARENT,
            egui::Color32::from_rgba_unmultiplied(112, 56, 4, 18),
        );

        let border = if pressed {
            egui::Color32::from_rgb(198, 116, 10)
        } else {
            mix(
                egui::Color32::from_rgb(216, 133, 16),
                egui::Color32::WHITE,
                hover * 0.10,
            )
        };
        painter.rect_stroke(
            body,
            radius,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );

        // Only a whisper of amber light on hover; the contact shadow does the
        // heavy lifting now.
        if hover > 0.01 && !pressed {
            glow(
                painter,
                body,
                radius,
                egui::Color32::from_rgba_unmultiplied(245, 158, 11, (9.0 * hover) as u8),
                18,
            );
        }

        paint_primary_label(painter, body, text, font_size, glyph, pressed);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    } else {
        painter.rect(
            rect,
            radius,
            egui::Color32::from_rgb(248, 238, 224),
            egui::Stroke::new(1.0, LINE),
            egui::StrokeKind::Inside,
        );
        let inner = rect.shrink(1.0);
        let inset = radius + 1.0;
        painter.line_segment(
            [
                egui::pos2(inner.left() + inset, inner.top() + 0.5),
                egui::pos2(inner.right() - inset, inner.top() + 0.5),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)),
        );
        paint_primary_label_disabled(painter, rect, text, font_size, glyph);
    }
    response.clicked()
}

fn paint_primary_label(
    painter: &egui::Painter,
    body: egui::Rect,
    text: &str,
    font_size: f32,
    glyph: PrimaryGlyph,
    pressed: bool,
) {
    let font = egui::FontId::new(font_size, crate::bold_family());
    let galley = painter.layout_no_wrap(text.to_owned(), font, INK);
    let icon_w = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        15.0
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
            egui::vec2(icon_w, 16.0),
        );
        paint_primary_glyph(
            painter,
            icon_rect,
            glyph,
            egui::Color32::from_rgba_unmultiplied(46, 36, 24, 218),
        );
    }
    let pos = egui::pos2(
        left + icon_w + gap,
        body.center().y - galley.size().y * 0.5 + 0.5 + if pressed { 0.5 } else { 0.0 },
    );
    painter.galley(pos, galley, INK);
}

fn paint_primary_label_disabled(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    font_size: f32,
    glyph: PrimaryGlyph,
) {
    let color = egui::Color32::from_rgb(158, 137, 112);
    let font = egui::FontId::new(font_size, crate::bold_family());
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let icon_w = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        15.0
    };
    let gap = if glyph == PrimaryGlyph::None {
        0.0
    } else {
        8.0
    };
    let total = icon_w + gap + galley.size().x;
    let left = rect.center().x - total * 0.5;
    if glyph != PrimaryGlyph::None {
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(left + icon_w * 0.5, rect.center().y),
            egui::vec2(icon_w, 16.0),
        );
        paint_primary_glyph(painter, icon_rect, glyph, color);
    }
    painter.galley(
        egui::pos2(
            left + icon_w + gap,
            rect.center().y - galley.size().y * 0.5 + 0.5,
        ),
        galley,
        color,
    );
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
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(c.x - 2.8, c.y - 5.0),
                    egui::pos2(c.x + 5.0, c.y),
                    egui::pos2(c.x - 2.8, c.y + 5.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        PrimaryGlyph::Download => {
            let stroke = egui::Stroke::new(1.7, color);
            painter.line_segment(
                [egui::pos2(c.x, c.y - 5.0), egui::pos2(c.x, c.y + 2.8)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - 3.6, c.y - 0.8), egui::pos2(c.x, c.y + 2.8)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x + 3.6, c.y - 0.8), egui::pos2(c.x, c.y + 2.8)],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(c.x - 5.0, c.y + 5.3),
                    egui::pos2(c.x + 5.0, c.y + 5.3),
                ],
                stroke,
            );
        }
    }
}

/// Secondary action: white surface, warm hairline border and a one-pixel
/// contact shadow. Keeps the primary amber button clearly in front.
pub fn secondary_sized(
    ui: &mut egui::Ui,
    text: &str,
    enabled: bool,
    width: f32,
    height: f32,
    font_size: f32,
) -> egui::Response {
    let desired = egui::vec2(width.max(48.0), height.max(24.0));
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() && enabled,
        0.14,
    );
    let pressed = enabled && response.is_pointer_button_down_on();
    let radius = (height * 0.24).clamp(7.0, 10.0);
    let painter = ui.painter();
    let body = rect.translate(egui::vec2(
        0.0,
        -hover * 0.5 + if pressed { 0.5 } else { 0.0 },
    ));
    if enabled {
        painter.add(
            Shadow {
                offset: [0, if pressed { 1 } else { 2 }],
                blur: if pressed { 3 } else { 7 },
                spread: 0,
                color: egui::Color32::from_rgba_unmultiplied(
                    128,
                    95,
                    60,
                    (14.0 + 6.0 * hover) as u8,
                ),
            }
            .as_shape(body, radius),
        );
        let fill = if pressed {
            crate::SURFACE2
        } else {
            mix(
                egui::Color32::WHITE,
                egui::Color32::from_rgb(255, 248, 236),
                hover,
            )
        };
        let border = mix(
            egui::Color32::from_rgb(231, 211, 184),
            crate::ACCENT_HI,
            hover * 0.7,
        );
        painter.rect(
            body,
            radius,
            fill,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        let inner = body.shrink(1.0);
        let inset = radius + 1.0;
        painter.line_segment(
            [
                egui::pos2(inner.left() + inset, inner.top() + 0.5),
                egui::pos2(inner.right() - inset, inner.top() + 0.5),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(84)),
        );
        painter.text(
            body.center() + egui::vec2(0.0, 0.5),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(font_size),
            INK,
        );
    } else {
        painter.rect(
            body,
            radius,
            egui::Color32::from_rgb(250, 243, 233),
            egui::Stroke::new(1.0, LINE),
            egui::StrokeKind::Inside,
        );
        painter.text(
            body.center() + egui::vec2(0.0, 0.5),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(font_size),
            MUTED,
        );
    }
    if enabled && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Custom progress track with a gradient fill and a slow sheen.
pub fn progress_bar(ui: &mut egui::Ui, fraction: f32, indeterminate: bool, time: f64) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 10.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        5,
        SURFACE2,
        egui::Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    let track = rect.shrink(1.0);
    let fill_rect = if indeterminate {
        let cycle = ((time as f32 * 0.55) % 1.0).clamp(0.0, 1.0);
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
        gradient_rrect(painter, fill_rect, 5.0, ACCENT_HI, ACCENT_LO);
        // Sheen sweeps across the filled part. Clipped to the fill so the track
        // never lights up ahead of progress.
        let clip = painter.with_clip_rect(fill_rect.intersect(track));
        let cycle = ((time as f32 * 0.45) % 1.0).clamp(0.0, 1.0);
        let sheen_w = 64.0;
        let x = fill_rect.left() - sheen_w + (fill_rect.width() + sheen_w * 2.0) * cycle;
        clip.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, fill_rect.top()),
                egui::pos2(x + sheen_w, fill_rect.bottom()),
            ),
            3,
            egui::Color32::from_white_alpha(70),
        );
    }
    if !indeterminate && fraction >= 0.995 {
        glow(
            painter,
            fill_rect,
            5,
            egui::Color32::from_rgba_unmultiplied(245, 158, 11, 28),
            10,
        );
    }
    // Tiny baseline pin so the empty track still reads as a component.
    painter.circle_filled(egui::pos2(track.right() - 2.0, track.center().y), 1.0, LINE);
}
