//! The home hero: an isometric voxel island, painted with flat block faces.
//!
//! This is deliberately not a photograph or a gradient panorama. Every shape
//! is an integer-aligned diamond or rectangle, so the artwork shares the same
//! geometry as the rest of the UI. It is painted only on repaint (there is no
//! continuous animation), keeping idle CPU at zero.

use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};

const GRASS_TOP: Color32 = Color32::from_rgb(107, 190, 74);
const GRASS_L: Color32 = Color32::from_rgb(84, 158, 56);
const GRASS_R: Color32 = Color32::from_rgb(67, 133, 45);
const DIRT_TOP: Color32 = Color32::from_rgb(139, 90, 43);
const DIRT_L: Color32 = Color32::from_rgb(116, 74, 36);
const DIRT_R: Color32 = Color32::from_rgb(94, 58, 28);
const STONE_TOP: Color32 = Color32::from_rgb(112, 116, 121);
const STONE_L: Color32 = Color32::from_rgb(88, 92, 97);
const STONE_R: Color32 = Color32::from_rgb(70, 74, 79);
const WATER_TOP: Color32 = Color32::from_rgb(74, 150, 219);
const WATER_L: Color32 = Color32::from_rgb(51, 112, 176);
const WATER_R: Color32 = Color32::from_rgb(38, 88, 145);
const WOOD: Color32 = Color32::from_rgb(112, 76, 44);
const WOOD_DARK: Color32 = Color32::from_rgb(78, 51, 29);
const LEAF_TOP: Color32 = Color32::from_rgb(67, 137, 57);
const LEAF_L: Color32 = Color32::from_rgb(51, 112, 45);
const LEAF_R: Color32 = Color32::from_rgb(40, 91, 37);
const GOLD: Color32 = Color32::from_rgb(242, 193, 78);
const GOLD_L: Color32 = Color32::from_rgb(255, 220, 120);
const GOLD_R: Color32 = Color32::from_rgb(188, 143, 45);
const SKY_TOP: Color32 = Color32::from_rgb(12, 16, 23);
const SKY_BOTTOM: Color32 = Color32::from_rgb(24, 33, 47);
const EDGE: Color32 = Color32::from_rgb(6, 7, 9);

pub fn paint_hero(painter: &egui::Painter, rect: Rect) {
    if rect.width() < 200.0 || rect.height() < 160.0 {
        return;
    }
    let split = rect.left() + rect.width() * 0.55;
    let left = Rect::from_min_max(rect.min, egui::pos2(split, rect.bottom()));
    let art = Rect::from_min_max(egui::pos2(split + 2.0, rect.top()), rect.max);

    // Left: flat stone plate with a quiet 16 px grid.
    painter.rect_filled(left, 0.0, crate::CARD);
    let mut x = left.left() + 16.0;
    while x < left.right() {
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(x, left.top()),
                egui::pos2(x + 1.0, left.bottom()),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 7),
        );
        x += 32.0;
    }
    let mut y = left.top() + 16.0;
    while y < left.bottom() {
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(left.left(), y),
                egui::pos2(left.right(), y + 1.0),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 7),
        );
        y += 32.0;
    }
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(split, rect.top()),
            egui::pos2(split + 2.0, rect.bottom()),
        ),
        0.0,
        EDGE,
    );

    let clipped = painter.with_clip_rect(art);
    paint_sky(&clipped, art);
    let unit = (art.width() / 17.0)
        .min(art.height() / 15.0)
        .clamp(12.0, 24.0)
        .round();
    paint_island(&clipped, art, unit);
    // A hard frame around the whole hero, on top of the art.
    painter.rect_stroke(rect, 0.0, Stroke::new(2.0, EDGE), egui::StrokeKind::Inside);
    painter.rect_stroke(
        rect.shrink(2.0),
        0.0,
        Stroke::new(1.0, Color32::from_rgb(64, 72, 82)),
        egui::StrokeKind::Inside,
    );
}

fn paint_sky(painter: &egui::Painter, art: Rect) {
    let bands = 9;
    for i in 0..bands {
        let t = i as f32 / (bands - 1) as f32;
        let top = mix(SKY_TOP, SKY_BOTTOM, t);
        let y0 = art.top() + art.height() * i as f32 / bands as f32;
        let y1 = art.top() + art.height() * (i + 1) as f32 / bands as f32;
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(art.left(), y0),
                egui::pos2(art.right(), y1 + 1.0),
            ),
            0.0,
            top,
        );
    }
    // Square moon with two-pixel halo.
    let moon = Rect::from_min_size(
        egui::pos2(
            art.right() - art.width() * 0.17,
            art.top() + art.height() * 0.10,
        ),
        egui::vec2(30.0, 30.0).round(),
    );
    painter.rect_filled(moon.expand(4.0), 0.0, Color32::from_rgb(30, 42, 58));
    painter.rect_filled(moon.expand(2.0), 0.0, Color32::from_rgb(208, 203, 177));
    painter.rect_filled(moon, 0.0, Color32::from_rgb(238, 232, 203));
    painter.rect_filled(
        Rect::from_min_size(moon.min + egui::vec2(6.0, 8.0), egui::vec2(5.0, 5.0)),
        0.0,
        Color32::from_rgb(203, 197, 171),
    );
    painter.rect_filled(
        Rect::from_min_size(moon.min + egui::vec2(17.0, 17.0), egui::vec2(4.0, 4.0)),
        0.0,
        Color32::from_rgb(203, 197, 171),
    );
    // Pixel stars: 2 x 2, sparse.
    let mut seed = 0x1BADB002u32;
    for _ in 0..28 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sx = art.left() + (seed >> 8) as f32 / 16_777_216.0 * art.width();
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sy = art.top() + (seed >> 8) as f32 / 16_777_216.0 * art.height() * 0.62;
        painter.rect_filled(
            Rect::from_min_size(egui::pos2(sx.round(), sy.round()), egui::vec2(2.0, 2.0)),
            0.0,
            Color32::from_rgb(206, 196, 170),
        );
    }
    // Two blocky clouds.
    cloud(
        painter,
        egui::pos2(art.left() + art.width() * 0.18, art.top() + 42.0),
        1.0,
    );
    cloud(
        painter,
        egui::pos2(art.left() + art.width() * 0.62, art.top() + 24.0),
        0.75,
    );
    // Distant stepped mountains.
    let base = art.bottom();
    let mut steps = vec![
        (0.00, 0.78),
        (0.08, 0.72),
        (0.16, 0.80),
        (0.24, 0.70),
        (0.33, 0.76),
        (0.42, 0.68),
        (0.52, 0.75),
        (0.61, 0.69),
        (0.70, 0.77),
        (0.80, 0.71),
        (0.90, 0.79),
        (1.00, 0.74),
    ];
    steps.push((1.0, 1.0));
    steps.push((0.0, 1.0));
    for pair in steps.windows(2) {
        let (ax, ay) = pair[0];
        let (bx, by) = pair[1];
        let y0 = base - art.height() * ay + 30.0;
        let y1 = base - art.height() * by + 30.0;
        let top = y0.min(y1);
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(art.left() + art.width() * ax, top),
                egui::pos2(art.left() + art.width() * bx, base),
            ),
            0.0,
            Color32::from_rgb(9, 13, 20),
        );
    }
}

fn cloud(painter: &egui::Painter, pos: Pos2, scale: f32) {
    let s = (14.0 * scale).round();
    let color = Color32::from_rgb(38, 46, 60);
    let edge = Color32::from_rgb(28, 35, 46);
    for (dx, dy, w) in [(0.0, 0.0, 4.0), (1.0, -1.0, 3.0), (2.0, 0.0, 3.0)] {
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(pos.x + dx * s, pos.y + dy * s),
                egui::vec2(w * s, s),
            ),
            0.0,
            color,
        );
    }
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(pos.x, pos.y + s), egui::vec2(5.0 * s, 2.0)),
        0.0,
        edge,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Grass,
    Water,
    Hill,
    Stone,
}

fn paint_island(painter: &egui::Painter, art: Rect, unit: f32) {
    let w = unit;
    let h = (unit * 0.5).round();
    let d = unit.round();
    let n = 7i32;
    let cx = art.center().x;
    let cy = art.bottom() - 13.0 * h - 4.0 * d - 10.0;
    let mut cells = vec![vec![Cell::Grass; n as usize]; n as usize];
    // A small pond and a raised hill for the tree.
    for (x, y) in [(4, 4), (4, 5), (5, 4)] {
        cells[y][x] = Cell::Water;
    }
    cells[2][2] = Cell::Hill;
    cells[1][5] = Cell::Stone;
    cells[5][1] = Cell::Stone;

    // Deepest taper first.
    for (layer, radius, color_top, color_l, color_r) in [
        (4.0, 0, STONE_TOP, STONE_L, STONE_R),
        (3.0, 1, STONE_TOP, STONE_L, STONE_R),
        (2.0, 2, STONE_TOP, STONE_L, STONE_R),
    ] {
        for (gy, row) in cells.iter().enumerate() {
            for (gx, _) in row.iter().enumerate() {
                let dist = (gx as i32 - 3).abs().max((gy as i32 - 3).abs());
                if dist > radius {
                    continue;
                }
                let (sx, sy) = iso_pos(cx, cy, w, h, d, gx as i32, gy as i32, layer);
                cube(painter, sx, sy, w, h, d, color_top, color_l, color_r);
            }
        }
    }
    // Dirt skirt across the full footprint.
    for (gy, row) in cells.iter().enumerate() {
        for (gx, cell) in row.iter().enumerate() {
            let (sx, sy) = iso_pos(cx, cy, w, h, d, gx as i32, gy as i32, 1.0);
            if *cell == Cell::Water {
                cube(painter, sx, sy, w, h, d, WATER_TOP, WATER_L, WATER_R);
            } else {
                cube(painter, sx, sy, w, h, d, DIRT_TOP, DIRT_L, DIRT_R);
            }
        }
    }
    // Top surface, back-to-front.
    for (gy, row) in cells.iter().enumerate() {
        for (gx, cell) in row.iter().enumerate() {
            let (sx, sy) = iso_pos(cx, cy, w, h, d, gx as i32, gy as i32, 0.0);
            match cell {
                Cell::Grass | Cell::Hill => {
                    cube(painter, sx, sy, w, h, d, GRASS_TOP, GRASS_L, GRASS_R);
                    if (gx + gy) % 3 == 0 {
                        dither_top(painter, sx, sy - h + 3.0, w, h, GRASS_TOP);
                    }
                }
                Cell::Water => {
                    cube(painter, sx, sy, w, h, d, WATER_TOP, WATER_L, WATER_R);
                    painter.rect_filled(
                        Rect::from_min_size(
                            egui::pos2(sx - w * 0.4, sy - h * 0.4),
                            egui::vec2(4.0, 2.0),
                        ),
                        0.0,
                        Color32::from_rgb(160, 213, 245),
                    );
                }
                Cell::Stone => {
                    cube(painter, sx, sy, w, h, d, STONE_TOP, STONE_L, STONE_R);
                    painter.rect_filled(
                        Rect::from_min_size(egui::pos2(sx - 3.0, sy - 2.0), egui::vec2(3.0, 3.0)),
                        0.0,
                        GOLD_L,
                    );
                    painter.rect_filled(
                        Rect::from_min_size(egui::pos2(sx + 2.0, sy + 1.0), egui::vec2(2.0, 2.0)),
                        0.0,
                        GOLD,
                    );
                }
            }
        }
    }
    // A thick stepped underside. Drawn after the top grid so the outer
    // silhouette reads as one carved block, not a paper-thin plate.
    paint_skirt(painter, cx, cy, w, h, d, n);

    // The hill's extra block plus a tree.
    let (hx, hy) = iso_pos(cx, cy, w, h, d, 2, 2, -1.0);
    cube(painter, hx, hy, w, h, d, GRASS_TOP, GRASS_L, GRASS_R);
    paint_tree(painter, hx, hy, unit);
    // A tiny explorer on the front edge.
    let (px, py) = iso_pos(cx, cy, w, h, d, 3, 5, 0.0);
    paint_character(painter, px, py - 10.0);
    // Floating ore blocks around the island.
    let floaters = [
        (
            art.left() + art.width() * 0.18,
            art.top() + art.height() * 0.44,
            0.55,
        ),
        (
            art.right() - art.width() * 0.12,
            art.top() + art.height() * 0.34,
            0.42,
        ),
        (
            art.left() + art.width() * 0.76,
            art.top() + art.height() * 0.22,
            0.32,
        ),
    ];
    for (fx, fy, scale) in floaters {
        let s = (unit * scale).max(4.0);
        iso_small_cube(painter, egui::pos2(fx, fy), s);
    }
}

fn paint_skirt(painter: &egui::Painter, cx: f32, cy: f32, w: f32, h: f32, d: f32, n: i32) {
    let top_c = cy + (n as f32 - 1.0) * h;
    let big_w = (n as f32) * w;
    let big_h = (n as f32) * h;
    let mut inset = 0.0;
    for band in 0..4 {
        let (fill_l, fill_r) = match band {
            0 => (DIRT_L, DIRT_R),
            1 => (STONE_L, STONE_R),
            2 => (Color32::from_rgb(76, 80, 85), Color32::from_rgb(60, 64, 69)),
            _ => (Color32::from_rgb(62, 66, 71), Color32::from_rgb(48, 52, 57)),
        };
        let top_y = top_c + band as f32 * d;
        let bot_y = top_y + d;
        let next_inset = inset + if band == 0 { 0.0 } else { w * 0.35 };
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(cx - big_w + inset, top_y),
                egui::pos2(cx, top_y + big_h),
                egui::pos2(cx, bot_y + big_h),
                egui::pos2(cx - big_w + next_inset, bot_y),
            ],
            fill_l,
            Stroke::new(1.0, EDGE),
        ));
        painter.add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(cx + big_w - inset, top_y),
                egui::pos2(cx, top_y + big_h),
                egui::pos2(cx, bot_y + big_h),
                egui::pos2(cx + big_w - next_inset, bot_y),
            ],
            fill_r,
            Stroke::new(1.0, EDGE),
        ));
        inset = next_inset;
    }
    // One-pixel dark seam between bands.
    for band in 1..4 {
        let y = top_c + band as f32 * d;
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(cx - big_w + inset * 0.5, y - 1.0),
                egui::vec2(big_w * 2.0, 1.0),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, 90),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn iso_pos(cx: f32, cy: f32, w: f32, h: f32, d: f32, gx: i32, gy: i32, layer: f32) -> (f32, f32) {
    let _ = (w, h, d);
    (
        cx + (gx - gy) as f32 * w,
        cy + (gx + gy) as f32 * h + layer * d,
    )
}

#[allow(clippy::too_many_arguments)]
fn cube(
    painter: &egui::Painter,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    d: f32,
    top: Color32,
    left: Color32,
    right: Color32,
) {
    let cx = cx.round();
    let cy = cy.round();
    let w = w.round();
    let h = h.round();
    let d = d.round();
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - w, cy),
            egui::pos2(cx, cy + h),
            egui::pos2(cx, cy + h + d),
            egui::pos2(cx - w, cy + d),
        ],
        left,
        Stroke::new(1.0, EDGE),
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx + w, cy),
            egui::pos2(cx, cy + h),
            egui::pos2(cx, cy + h + d),
            egui::pos2(cx + w, cy + d),
        ],
        right,
        Stroke::new(1.0, EDGE),
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx, cy - h),
            egui::pos2(cx + w, cy),
            egui::pos2(cx, cy + h),
            egui::pos2(cx - w, cy),
        ],
        top,
        Stroke::new(1.0, EDGE),
    ));
}

fn dither_top(painter: &egui::Painter, _x: f32, _y: f32, _w: f32, _h: f32, color: Color32) {
    let _ = (painter, color);
}

fn paint_tree(painter: &egui::Painter, x: f32, y: f32, unit: f32) {
    let x = x.round();
    let base = y.round();
    let trunk_w = (unit * 0.34).round().max(3.0);
    let trunk_h = (unit * 1.55).round();
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(x - trunk_w * 0.5, base - trunk_h),
            egui::vec2(trunk_w, trunk_h),
        ),
        0.0,
        WOOD,
    );
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(x - trunk_w * 0.5, base - trunk_h),
            egui::vec2(1.0, trunk_h),
        ),
        0.0,
        Color32::from_rgb(140, 99, 58),
    );
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(x + trunk_w * 0.5 - 1.0, base - trunk_h),
            egui::vec2(1.0, trunk_h),
        ),
        0.0,
        WOOD_DARK,
    );
    let lw = (unit * 0.72).round().max(5.0);
    let lh = (lw * 0.5).round();
    let ld = (lw * 0.55).round();
    let top = base - trunk_h - lh * 0.5;
    // Outer blocks first, centre and crown last.
    cube(
        painter,
        x - lw * 0.95,
        top + lh * 0.55,
        lw,
        lh,
        ld,
        LEAF_L,
        LEAF_R,
        LEAF_R,
    );
    cube(
        painter,
        x + lw * 0.95,
        top + lh * 0.55,
        lw,
        lh,
        ld,
        LEAF_L,
        LEAF_R,
        LEAF_R,
    );
    cube(painter, x, top, lw, lh, ld, LEAF_TOP, LEAF_L, LEAF_R);
    cube(
        painter,
        x,
        top - lh * 0.95,
        lw * 0.75,
        lh * 0.75,
        ld * 0.75,
        LEAF_TOP,
        LEAF_L,
        LEAF_R,
    );
}

fn paint_character(painter: &egui::Painter, x: f32, y: f32) {
    let x = x.round();
    let y = y.round();
    let s = 2.0;
    // shadow
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x - s * 3.0, y + s * 5.0), egui::vec2(s * 6.0, s)),
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, 90),
    );
    // legs
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x - s * 1.5, y + s * 3.0), egui::vec2(s, s * 2.0)),
        0.0,
        Color32::from_rgb(46, 62, 115),
    );
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x + s * 0.5, y + s * 3.0), egui::vec2(s, s * 2.0)),
        0.0,
        Color32::from_rgb(38, 53, 99),
    );
    // body
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x - s * 1.5, y + s), egui::vec2(s * 3.0, s * 2.0)),
        0.0,
        Color32::from_rgb(74, 170, 160),
    );
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x - s * 1.5, y + s), egui::vec2(s, s * 2.0)),
        0.0,
        Color32::from_rgb(94, 194, 181),
    );
    // arms
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x - s * 2.5, y + s), egui::vec2(s, s * 2.0)),
        0.0,
        Color32::from_rgb(198, 146, 98),
    );
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x + s * 1.5, y + s), egui::vec2(s, s * 2.0)),
        0.0,
        Color32::from_rgb(178, 128, 82),
    );
    // head
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(x - s * 1.5, y - s * 2.0),
            egui::vec2(s * 3.0, s * 3.0),
        ),
        0.0,
        Color32::from_rgb(211, 160, 110),
    );
    painter.rect_filled(
        Rect::from_min_size(egui::pos2(x - s * 1.5, y - s * 2.0), egui::vec2(s * 3.0, s)),
        0.0,
        Color32::from_rgb(83, 55, 34),
    );
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(x - s * 1.0, y - s * 0.5),
            egui::vec2(s * 0.5, s * 0.5),
        ),
        0.0,
        Color32::from_rgb(40, 30, 25),
    );
    painter.rect_filled(
        Rect::from_min_size(
            egui::pos2(x + s * 0.5, y - s * 0.5),
            egui::vec2(s * 0.5, s * 0.5),
        ),
        0.0,
        Color32::from_rgb(40, 30, 25),
    );
}

fn iso_small_cube(painter: &egui::Painter, center: Pos2, size: f32) {
    let w = size.round().max(3.0);
    let h = (w * 0.5).round().max(2.0);
    let d = w.max(3.0);
    let x = center.x.round();
    let y = center.y.round();
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - w, y),
            egui::pos2(x, y + h),
            egui::pos2(x, y + h + d),
            egui::pos2(x - w, y + d),
        ],
        GOLD_R,
        Stroke::new(1.0, EDGE),
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x + w, y),
            egui::pos2(x, y + h),
            egui::pos2(x, y + h + d),
            egui::pos2(x + w, y + d),
        ],
        GOLD,
        Stroke::new(1.0, EDGE),
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x, y - h),
            egui::pos2(x + w, y),
            egui::pos2(x, y + h),
            egui::pos2(x - w, y),
        ],
        GOLD_L,
        Stroke::new(1.0, EDGE),
    ));
}

/// A small isometric gold block used as the launcher mark.
pub fn paint_gold_block(painter: &egui::Painter, rect: Rect) {
    let s = rect.width().min(rect.height());
    let cx = rect.center().x.round();
    let top = rect.center().y - s * 0.42;
    let w = s * 0.5;
    let h = s * 0.25;
    let d = s * 0.28;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx - w, top),
            egui::pos2(cx, top + h),
            egui::pos2(cx, top + h + d),
            egui::pos2(cx - w, top + d),
        ],
        GOLD_R,
        Stroke::new(1.0, EDGE),
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx + w, top),
            egui::pos2(cx, top + h),
            egui::pos2(cx, top + h + d),
            egui::pos2(cx + w, top + d),
        ],
        GOLD,
        Stroke::new(1.0, EDGE),
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(cx, top - h),
            egui::pos2(cx + w, top),
            egui::pos2(cx, top + h),
            egui::pos2(cx - w, top),
        ],
        GOLD_L,
        Stroke::new(1.0, EDGE),
    ));
}

/// A 26 px grass-block badge used by the world slot and quick actions.
pub fn paint_grass_block(painter: &egui::Painter, rect: Rect) {
    let r = rect.shrink(1.0);
    painter.rect_filled(rect, 0.0, EDGE);
    painter.rect_filled(r, 0.0, Color32::from_rgb(107, 190, 74));
    let dirt_top = r.top() + r.height() * 0.42;
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(r.left(), dirt_top),
            egui::pos2(r.right(), r.bottom()),
        ),
        0.0,
        Color32::from_rgb(139, 90, 43),
    );
    painter.rect_filled(
        Rect::from_min_max(
            egui::pos2(r.left(), dirt_top - 2.0),
            egui::pos2(r.right(), dirt_top),
        ),
        0.0,
        Color32::from_rgb(67, 133, 45),
    );
    painter.rect_filled(
        Rect::from_min_size(r.left_top(), egui::vec2(r.width(), 2.0)),
        0.0,
        Color32::from_rgb(151, 214, 111),
    );
    for (fx, fy, w, h) in [
        (0.2, 0.55, 0.12, 0.1),
        (0.62, 0.72, 0.16, 0.1),
        (0.4, 0.86, 0.1, 0.08),
    ] {
        painter.rect_filled(
            Rect::from_min_size(
                egui::pos2(r.left() + r.width() * fx, r.top() + r.height() * fy),
                egui::vec2(r.width() * w, r.height() * h),
            ),
            0.0,
            Color32::from_rgb(104, 66, 31),
        );
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    crate::motion::mix(a, b, t)
}
