//! Procedural night-sky artwork for the shell and the home hero.
//!
//! The launcher does not bundle a panorama. Instead it paints a small,
//! deterministic scene once per size: aurora ribbons, a star field, a
//! constellation and stepped "block" ridges. The result is cached as a
//! texture, so the expensive per-pixel work never runs inside the frame loop
//! and idle CPU stays at zero.

use eframe::egui;
use egui::{Color32, ColorImage, TextureHandle, TextureOptions, Vec2};

use crate::motion;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    /// Quiet backdrop behind every page.
    Window,
    /// Rich artwork inside the home hero card.
    Hero,
}

pub struct Backdrop {
    size: [usize; 2],
    scene: Scene,
    texture: TextureHandle,
}

impl Backdrop {
    /// Make sure the cached texture matches the requested size and scene.
    /// Returns `true` when a new texture was uploaded this frame.
    pub fn ensure(
        ctx: &egui::Context,
        slot: &mut Option<Backdrop>,
        size: Vec2,
        scene: Scene,
    ) -> bool {
        let width = size.x.round().clamp(32.0, 4096.0) as usize;
        let height = size.y.round().clamp(32.0, 4096.0) as usize;
        if let Some(cache) = slot
            && cache.size == [width, height]
            && cache.scene == scene
        {
            return false;
        }
        let image = render(width, height, scene);
        let name = match scene {
            Scene::Window => "nailong-backdrop-window",
            Scene::Hero => "nailong-backdrop-hero",
        };
        let texture = ctx.load_texture(
            format!("{name}-{width}x{height}"),
            image,
            TextureOptions::LINEAR,
        );
        *slot = Some(Backdrop {
            size: [width, height],
            scene,
            texture,
        });
        true
    }

    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        painter.image(
            self.texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

#[derive(Clone, Copy)]
struct Star {
    x: f32,
    y: f32,
    size: f32,
    warm: f32,
    twinkle: f32,
}

fn rng(seed: &mut u32) -> f32 {
    // Small deterministic LCG; enough for scattered stars and ridge jitter.
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / 16_777_216.0
}

fn smooth(a: f32) -> f32 {
    a * a * (3.0 - 2.0 * a)
}

/// Value-noise lookup on a small precomputed grid, bilinear unlike the hash.
fn sample_noise(noise: &[f32], n: usize, x: f32, y: f32) -> f32 {
    let sx = (x * n as f32).rem_euclid(n as f32);
    let sy = (y * n as f32).rem_euclid(n as f32);
    let x0 = sx.floor() as usize % n;
    let y0 = sy.floor() as usize % n;
    let x1 = (x0 + 1) % n;
    let y1 = (y0 + 1) % n;
    let tx = smooth(sx.fract());
    let ty = smooth(sy.fract());
    let a = noise[y0 * n + x0] * (1.0 - tx) + noise[y0 * n + x1] * tx;
    let b = noise[y1 * n + x0] * (1.0 - tx) + noise[y1 * n + x1] * tx;
    a * (1.0 - ty) + b * ty
}

fn add(base: Color32, color: Color32, alpha: f32) -> Color32 {
    let a = alpha.clamp(0.0, 1.0);
    let blend = |x: u8, y: u8| -> u8 {
        (x as f32 + (y as f32 - x as f32) * a)
            .clamp(0.0, 255.0)
            .round() as u8
    };
    Color32::from_rgb(
        blend(base.r(), color.r()),
        blend(base.g(), color.g()),
        blend(base.b(), color.b()),
    )
}

/// Additive light, used for glows and stars so bright pixels bloom toward the
/// accent colour instead of replacing the sky.
fn lighten(base: Color32, color: Color32, amount: f32) -> Color32 {
    let r = (base.r() as f32 + color.r() as f32 * amount).min(255.0) as u8;
    let g = (base.g() as f32 + color.g() as f32 * amount).min(255.0) as u8;
    let b = (base.b() as f32 + color.b() as f32 * amount).min(255.0) as u8;
    Color32::from_rgb(r, g, b)
}

pub fn render(width: usize, height: usize, scene: Scene) -> ColorImage {
    let w = width.max(2);
    let h = height.max(2);
    let hero = scene == Scene::Hero;
    let horizon = if hero { 0.68 } else { 0.80 };
    let mut pixels = vec![Color32::BLACK; w * h];

    // A 64 x 64 noise field keeps the aurora organic without allocating a
    // full per-pixel hash.
    let n = 64usize;
    let mut seed = if hero { 0x51F0_A23B } else { 0x2C91_5D43 };
    let mut noise = Vec::with_capacity(n * n);
    for _ in 0..n * n {
        noise.push(rng(&mut seed));
    }

    let teal = Color32::from_rgb(64, 232, 199);
    let violet = Color32::from_rgb(139, 108, 255);
    let amber = Color32::from_rgb(255, 168, 76);
    let rose = Color32::from_rgb(255, 112, 154);

    // Deterministic star field. Hero gets a denser, brighter sky.
    let mut stars = Vec::with_capacity(if hero { 420 } else { 260 });
    let star_count = if hero { 140 } else { 90 };
    for _ in 0..star_count {
        let x = rng(&mut seed);
        let y = rng(&mut seed) * if hero { 0.72 } else { 0.84 };
        let size = if hero {
            0.35 + rng(&mut seed).powf(3.2) * 1.25
        } else {
            0.25 + rng(&mut seed).powf(3.2) * 0.85
        };
        let warm = rng(&mut seed);
        let twinkle = rng(&mut seed);
        stars.push(Star {
            x,
            y,
            size,
            warm,
            twinkle,
        });
    }

    let moon = if hero {
        Some((0.825f32, 0.205f32, 0.047f32))
    } else {
        Some((0.895f32, 0.115f32, 0.021f32))
    };

    // Per-column aurora centres (three ribbons). Computing them once cuts the
    // sin() calls from a per-pixel 2.4M+ down to a few thousand.
    let mut bands: Vec<[f32; 3]> = vec![[0.0; 3]; w];
    for (x, slot) in bands.iter_mut().enumerate() {
        let fx = x as f32 / (w - 1) as f32;
        let n1 = sample_noise(&noise, n, fx * 1.7, 0.23) - 0.5;
        let n2 = sample_noise(&noise, n, fx * 2.4 + 0.3, 0.71) - 0.5;
        *slot = [
            if hero { 0.30 } else { 0.22 } + fx * 0.07 + n1 * 0.085,
            if hero { 0.22 } else { 0.14 } + fx * 0.04 + n2 * 0.07,
            if hero { 0.40 } else { 0.32 } + fx * 0.03 + n1 * 0.06,
        ];
    }

    let inv_w = 1.0 / (w - 1) as f32;
    let inv_h = 1.0 / (h - 1) as f32;
    for y in 0..h {
        let fy = y as f32 * inv_h;
        for (x, band) in bands.iter().enumerate() {
            let fx = x as f32 * inv_w;
            let index = y * w + x;

            // Base: midnight with a cooler top and a warmer lower sky.
            let top = Color32::from_rgb(5, 8, 15);
            let bottom = if hero {
                Color32::from_rgb(10, 18, 31)
            } else {
                Color32::from_rgb(8, 13, 24)
            };
            let mut color = mix(top, bottom, fy.powf(0.82));

            // Very broad atmospheric washes anchor the scene to the palette.
            let wash = |cx: f32, cy: f32, radius: f32| {
                let dx = (fx - cx) * 1.45;
                let dy = fy - cy;
                (-(dx * dx + dy * dy) / (radius * radius)).exp()
            };
            let glow_strength = if hero { 1.0 } else { 0.55 };
            color = add(color, teal, wash(0.08, 0.06, 0.62) * 0.055 * glow_strength);
            color = add(color, violet, wash(0.72, 0.02, 0.46) * 0.05 * glow_strength);
            color = add(color, amber, wash(0.95, 0.62, 0.52) * 0.05 * glow_strength);

            // Aurora ribbons: a soft gaussian around each centre line.
            let ribbon = |cx: f32, width: f32| {
                let d = (fy - cx) / width;
                (-d * d).exp()
            };
            let band = *band;
            let fade = (1.0 - (fy / (horizon + 0.08)).powi(2)).clamp(0.0, 1.0);
            let shimmer = 0.62
                + 0.38
                    * (0.5
                        + 0.5 * (fx * 7.5 + band[0] * 11.0 + 0.6).sin() * (fx * 2.3 + 0.2).cos());
            let a1 = ribbon(band[0], 0.095) * fade * if hero { 0.22 } else { 0.07 };
            let a2 = ribbon(band[1], 0.075) * fade * if hero { 0.18 } else { 0.055 };
            let a3 = ribbon(band[2], 0.115) * fade * if hero { 0.11 } else { 0.04 };
            color = add(color, teal, a1 * shimmer);
            color = add(color, violet, a2 * shimmer);
            color = add(color, rose, a3 * 0.5);
            color = add(color, amber, a3 * 0.7);

            // Moon halo + disc.
            if let Some((mx, my, mr)) = moon {
                let dx = (fx - mx) * (w as f32 / h as f32);
                let dy = fy - my;
                let d = (dx * dx + dy * dy).sqrt();
                if d < mr * 6.0 {
                    let halo = (1.0 - (d / (mr * 6.0)).powf(0.72)).max(0.0);
                    color = lighten(
                        color,
                        Color32::from_rgb(255, 226, 174),
                        halo * halo * if hero { 0.23 } else { 0.10 },
                    );
                    if d < mr {
                        let edge = 1.0 - (d / mr).powi(12);
                        let moon_col = mix(
                            Color32::from_rgb(255, 244, 214),
                            Color32::from_rgb(255, 203, 132),
                            (d / mr).powi(2),
                        );
                        color = add(
                            color,
                            moon_col,
                            (edge * if hero { 0.95 } else { 0.75 }).clamp(0.0, 1.0),
                        );
                        let rx = (fx - mx) / mr;
                        let ry = (fy - my) / mr;
                        for (cx, cy, cr) in [
                            (-0.28f32, 0.18f32, 0.32f32),
                            (0.32, -0.22, 0.22),
                            (0.05, 0.52, 0.16),
                        ] {
                            let dd = ((rx - cx).powi(2) + (ry - cy).powi(2)).sqrt();
                            if dd < cr {
                                color = mix(
                                    color,
                                    Color32::from_rgb(218, 186, 139),
                                    (1.0 - dd / cr) * 0.45,
                                );
                            }
                        }
                    }
                }
            }

            // Stars are computed after the sky so they sit on top of aurora.
            // The loop below is intentionally outside this pixel (see below).
            pixels[index] = color;
        }
    }

    // Paint stars as small discs with a halo. Looping over each star's bounds
    // is far cheaper than checking every star for every pixel.
    for star in &stars {
        let sx = star.x * (w - 1) as f32;
        let sy = star.y * (h - 1) as f32;
        let base = if hero { 0.95 } else { 0.6 };
        let bright = (0.24 + star.twinkle * 0.56) * base;
        let tint = if star.warm > 0.72 {
            Color32::from_rgb(255, 228, 184)
        } else if star.warm < 0.22 {
            Color32::from_rgb(198, 226, 255)
        } else {
            Color32::from_rgb(235, 241, 255)
        };
        let radius = star.size * 2.1;
        let x0 = (sx - radius).floor().max(0.0) as usize;
        let x1 = (sx + radius).ceil().min((w - 1) as f32) as usize;
        let y0 = (sy - radius).floor().max(0.0) as usize;
        let y1 = (sy + radius).ceil().min((h - 1) as f32) as usize;
        for py in y0..=y1 {
            for px in x0..=x1 {
                let dx = px as f32 - sx;
                let dy = py as f32 - sy;
                let d = (dx * dx + dy * dy).sqrt();
                if d > radius {
                    continue;
                }
                let core = (1.0 - d / radius).powf(1.8);
                let idx = py * w + px;
                pixels[idx] = lighten(pixels[idx], tint, core * bright * 1.05);
            }
        }
        if hero && star.size > 1.08 {
            // Four-point glint; only a handful of stars qualify, so the sky
            // keeps a few landmarks without looking like bokeh.
            for step in 1..=4 {
                let fade = 0.09 * (1.0 - step as f32 / 5.0);
                for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                    let px = sx as isize + dx;
                    let py = sy as isize + dy;
                    if px >= 0 && py >= 0 && px < w as isize && py < h as isize {
                        let idx = py as usize * w + px as usize;
                        pixels[idx] = lighten(pixels[idx], tint, fade);
                    }
                }
            }
        }
        // A slightly larger, almost invisible bloom around the brighter stars.
        if star.size > 1.35 && hero {
            for py in y0..=y1 {
                for px in x0..=x1 {
                    let dx = px as f32 - sx;
                    let dy = py as f32 - sy;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d <= radius {
                        continue;
                    }
                    let halo = (1.0 - (d - radius) / (radius * 1.8)).max(0.0);
                    if halo > 0.0 {
                        let idx = py * w + px;
                        pixels[idx] = lighten(pixels[idx], tint, halo * 0.018 * bright);
                    }
                }
            }
        }
    }

    // The dragon constellation: a quiet line of stars over the right-hand sky.
    // Only the hero gets it; on inner pages it would be covered by cards.
    if hero {
        let points = [
            (0.555, 0.505),
            (0.610, 0.455),
            (0.665, 0.478),
            (0.722, 0.408),
            (0.779, 0.430),
            (0.836, 0.360),
            (0.893, 0.382),
            (0.934, 0.326),
        ];
        let gold = Color32::from_rgb(255, 222, 168);
        for pair in points.windows(2) {
            let a = egui::pos2(pair[0].0 * w as f32, pair[0].1 * h as f32);
            let b = egui::pos2(pair[1].0 * w as f32, pair[1].1 * h as f32);
            draw_line(&mut pixels, w, h, a, b, gold, 0.16);
        }
        for (index, point) in points.iter().enumerate() {
            let cx = point.0 * w as f32;
            let cy = point.1 * h as f32;
            let radius = if index == 0 { 4.4 } else { 2.4 };
            for py in (cy - 6.0).max(0.0) as usize..=(cy + 6.0).min((h - 1) as f32) as usize {
                for px in (cx - 6.0).max(0.0) as usize..=(cx + 6.0).min((w - 1) as f32) as usize {
                    let dx = px as f32 - cx;
                    let dy = py as f32 - cy;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d <= radius {
                        let idx = py * w + px;
                        let amount = (1.0 - d / radius) * 0.85;
                        pixels[idx] = lighten(pixels[idx], gold, amount);
                    }
                }
            }
        }
    }

    // Three stepped ridges. Each layer is slightly lighter than the one in
    // front of it, which reads as fog between the hills.
    let layers = if hero {
        [
            (0.665, 0.050, Color32::from_rgb(17, 31, 50), 0.0),
            (0.745, 0.060, Color32::from_rgb(12, 24, 41), 1.7),
            (0.845, 0.075, Color32::from_rgb(9, 17, 31), 4.9),
        ]
    } else {
        [
            (0.820, 0.030, Color32::from_rgb(12, 22, 38), 0.0),
            (0.885, 0.038, Color32::from_rgb(9, 17, 31), 2.1),
            (0.950, 0.045, Color32::from_rgb(7, 13, 24), 5.3),
        ]
    };
    let block = if hero { 7.0 } else { 5.0 };
    for (layer, (base, amp, color, phase)) in layers.into_iter().enumerate() {
        let mut tops = Vec::with_capacity(w);
        for x in 0..w {
            let fx = x as f32 * inv_w;
            let wobble = (fx * (7.0 + layer as f32 * 2.4) + phase).sin() * 0.35
                + (fx * (17.0 - layer as f32) + phase * 0.7).sin() * 0.18
                + (sample_noise(
                    &noise,
                    n,
                    fx * 3.0 + layer as f32 * 0.13,
                    0.61 + layer as f32 * 0.11,
                ) - 0.5)
                    * 0.9;
            let raw = (base + amp * wobble) * h as f32;
            let stepped = ((raw / block).round() * block).clamp(0.0, h as f32 - 2.0);
            tops.push(stepped as usize);
        }
        for (x, &top) in tops.iter().enumerate() {
            let rim_col = if hero && layer == 0 {
                Color32::from_rgb(40, 68, 96)
            } else {
                Color32::from_rgb(28, 49, 74)
            };
            for y in top..h {
                let idx = y * w + x;
                let depth = (y - top) as f32 / (h - top).max(1) as f32;
                let shade = mix(color, Color32::from_rgb(4, 7, 13), depth * 0.55);
                pixels[idx] = shade;
            }
            if top < h {
                let idx = top * w + x;
                pixels[idx] = lighten(pixels[idx], rim_col, if layer == 0 { 0.55 } else { 0.3 });
            }
        }
        // A few blocky spruces on the nearest ridge.
        if layer == 2 {
            let mut tree_seed = seed ^ (0x9E37_79B9u32.wrapping_mul(layer as u32 + 1));
            let count = if hero { 16 } else { 10 };
            for _ in 0..count {
                let x = (rng(&mut tree_seed) * (w - 1) as f32) as usize;
                let top = tops[x];
                let trunk_h = if hero { 5.0 } else { 3.0 };
                let crown = if hero { 3.0 } else { 2.0 };
                let base_y = top as f32 - 1.0;
                let tree_col = Color32::from_rgb(5, 10, 19);
                let tree_rim = Color32::from_rgb(20, 38, 59);
                let trunk_x = x as f32;
                draw_block(
                    &mut pixels,
                    w,
                    h,
                    egui::Rect::from_min_size(
                        egui::pos2(trunk_x - 0.5, base_y - trunk_h),
                        egui::vec2(1.0, trunk_h),
                    ),
                    tree_col,
                );
                draw_block(
                    &mut pixels,
                    w,
                    h,
                    egui::Rect::from_min_size(
                        egui::pos2(trunk_x - crown, base_y - trunk_h - 1.5),
                        egui::vec2(crown * 2.0, 2.5),
                    ),
                    tree_col,
                );
                draw_block(
                    &mut pixels,
                    w,
                    h,
                    egui::Rect::from_min_size(
                        egui::pos2(trunk_x - crown + 1.0, base_y - trunk_h - 3.5),
                        egui::vec2(crown * 2.0 - 2.0, 2.5),
                    ),
                    tree_col,
                );
                let rim_y = (base_y - trunk_h - 4.0).max(0.0) as usize;
                if rim_y < h {
                    let px = x.min(w - 1);
                    pixels[rim_y * w + px] = tree_rim;
                }
            }
        }
    }

    // Vignette + film grain. Kept very subtle so text keeps its contrast.
    let vignette = if hero { 0.34 } else { 0.22 };
    for y in 0..h {
        let fy = y as f32 * inv_h;
        for x in 0..w {
            let fx = x as f32 * inv_w;
            let idx = y * w + x;
            let dx = (fx - 0.5) * 1.7;
            let dy = fy - 0.5;
            let edge = (dx * dx + dy * dy).sqrt().clamp(0.0, 1.4);
            let dark = (edge * edge * vignette).min(0.55);
            let grain_seed =
                (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663) ^ seed;
            let grain = ((grain_seed >> 7) & 7) as i16 - 4;
            let p = pixels[idx];
            pixels[idx] = Color32::from_rgb(
                ((p.r() as f32 * (1.0 - dark) + grain as f32).clamp(0.0, 255.0)) as u8,
                ((p.g() as f32 * (1.0 - dark) + grain as f32).clamp(0.0, 255.0)) as u8,
                ((p.b() as f32 * (1.0 - dark) + grain as f32).clamp(0.0, 255.0)) as u8,
            );
        }
    }

    // The hero carries a lot of text on its left half. Bake the scrim into
    // the artwork so the rounded corners stay clean (painting a rectangular
    // overlay in the UI would square them off again).
    if hero {
        let midnight = Color32::from_rgb(5, 9, 16);
        for y in 0..h {
            let fy = y as f32 * inv_h;
            for x in 0..w {
                let fx = x as f32 * inv_w;
                let left = (1.0 - (fx / 0.64).clamp(0.0, 1.0)).powf(0.72) * 0.88;
                let bottom = ((fy - 0.58) / 0.42).clamp(0.0, 1.0).powf(1.35) * 0.34;
                let scrim = left.max(bottom);
                let idx = y * w + x;
                pixels[idx] = mix(pixels[idx], midnight, scrim);
            }
        }
    }

    // Hero artwork lives inside a rounded card: bake transparent corners so
    // the card fill shows through instead of a hard rectangle.
    if hero {
        let radius = (24.0 * (w as f32 / 824.0)).clamp(18.0, 32.0);
        let r = radius;
        for y in 0..h {
            for x in 0..w {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let cx = if px < r {
                    r
                } else if px > w as f32 - r {
                    w as f32 - r
                } else {
                    px
                };
                let cy = if py < r {
                    r
                } else if py > h as f32 - r {
                    h as f32 - r
                } else {
                    py
                };
                let dx = px - cx;
                let dy = py - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                // Only the four corners can be outside the rounded rect.
                let alpha = if (px < r || px > w as f32 - r) && (py < r || py > h as f32 - r) {
                    ((r - dist) / 1.5).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                if alpha < 1.0 {
                    let p = pixels[y * w + x];
                    pixels[y * w + x] =
                        Color32::from_rgba_unmultiplied(p.r(), p.g(), p.b(), (alpha * 255.0) as u8);
                }
            }
        }
    }

    ColorImage {
        size: [w, h],
        source_size: Vec2::new(w as f32, h as f32),
        pixels,
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    motion::mix(a, b, t)
}

/// Bresenham-ish line with a soft, low-alpha colour. Used for the
/// constellation only, so a simple stepping loop is enough.
fn draw_line(
    pixels: &mut [Color32],
    w: usize,
    h: usize,
    a: egui::Pos2,
    b: egui::Pos2,
    color: Color32,
    alpha: f32,
) {
    let steps = (b - a).length().max(1.0) as usize;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = a.x + (b.x - a.x) * t;
        let y = a.y + (b.y - a.y) * t;
        let xi = x.round() as isize;
        let yi = y.round() as isize;
        if xi < 0 || yi < 0 || xi >= w as isize || yi >= h as isize {
            continue;
        }
        let idx = yi as usize * w + xi as usize;
        pixels[idx] = lighten(pixels[idx], color, alpha);
    }
}

/// Integer-aligned block fill. Coordinates are in logical pixels so the
/// silhouettes keep the launcher's deliberate "stepped" language.
fn draw_block(pixels: &mut [Color32], w: usize, h: usize, rect: egui::Rect, color: Color32) {
    let x0 = rect.left().round().max(0.0) as usize;
    let y0 = rect.top().round().max(0.0) as usize;
    let x1 = (rect.right().round() as usize).min(w);
    let y1 = (rect.bottom().round() as usize).min(h);
    for py in y0..y1 {
        for px in x0..x1 {
            pixels[py * w + px] = color;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_both_scenes_deterministically() {
        let hero = render(160, 96, Scene::Hero);
        assert_eq!(hero.size, [160, 96]);
        assert_eq!(hero.pixels.len(), 160 * 96);
        // Top-left corner is rounded away on the hero card.
        assert_eq!(hero.pixels[0].a(), 0);
        // The centre is opaque and not pure black.
        let centre = hero.pixels[48 * 160 + 80];
        assert_eq!(centre.a(), 255);
        assert!(centre.r() + centre.g() + centre.b() > 20);

        let window = render(80, 48, Scene::Window);
        assert_eq!(window.size, [80, 48]);
        assert_eq!(window.pixels.len(), 80 * 48);
        assert_eq!(window.pixels[0].a(), 255);
    }
}
