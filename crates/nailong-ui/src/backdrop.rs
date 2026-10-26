//! Deepslate shell texture.
//!
//! Every page sits on one deterministic, cached 48 x 24 brick field with
//! two-pixel seams and a sparse dither. No gradients, no stars, no noise
//! bloom: the shell reads as carved stone, not as a web gradient.

use eframe::egui;
use egui::{Color32, ColorImage, TextureHandle, TextureOptions, Vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    Window,
    Hero,
}

pub struct Backdrop {
    size: [usize; 2],
    scene: Scene,
    texture: TextureHandle,
}

impl Backdrop {
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
        let texture = ctx.load_texture(
            format!("nailong-deepslate-{width}x{height}"),
            image,
            TextureOptions::NEAREST,
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

fn rng(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as f32 / 16_777_216.0
}

pub fn render(width: usize, height: usize, scene: Scene) -> ColorImage {
    let w = width.max(2);
    let h = height.max(2);
    let base = if scene == Scene::Hero {
        Color32::from_rgb(22, 25, 29)
    } else {
        Color32::from_rgb(15, 17, 20)
    };
    let light = Color32::from_rgb(22, 25, 29);
    let dark = Color32::from_rgb(9, 10, 12);
    let seam = Color32::from_rgb(6, 7, 8);
    let mut pixels = vec![base; w * h];
    let mut seed = 0x51F0_A23Bu32;

    // 48 x 24 bricks, offset by half a brick on odd rows.
    let bw = 48usize;
    let bh = 24usize;
    let mut row = 0usize;
    let mut y = 0usize;
    while y < h {
        let offset = if row.is_multiple_of(2) { 0 } else { bw / 2 };
        let mut x = 0usize;
        while x < w {
            let x0 = x.saturating_sub(offset);
            let shade = (rng(&mut seed) * 3.0).floor() as i32;
            let fill = match shade {
                0 => base,
                1 => light,
                _ => Color32::from_rgb(18, 20, 24),
            };
            let x1 = (x + bw).min(w);
            let y1 = (y + bh).min(h);
            for py in y..y1 {
                for px in x0.max(x)..x1 {
                    pixels[py * w + px] = fill;
                }
            }
            // Two-pixel dark mortar on the right and bottom edges.
            for py in y..y1 {
                for px in x1.saturating_sub(2)..x1 {
                    if px >= x0 {
                        pixels[py * w + px] = seam;
                    }
                }
            }
            for py in y1.saturating_sub(2)..y1 {
                for px in x0.max(x)..x1 {
                    pixels[py * w + px] = seam;
                }
            }
            // Sparse 4 px dither inside the brick.
            let dither = (rng(&mut seed) * 6.0) as usize;
            for _ in 0..dither {
                let px = x0 + 8 + (rng(&mut seed) * 32.0) as usize;
                let py = y + 4 + (rng(&mut seed) * 14.0) as usize;
                if px < x1 - 3 && py < y1 - 3 {
                    let color = if rng(&mut seed) > 0.5 { dark } else { light };
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let idx = (py + dy) * w + px + dx;
                            if idx < pixels.len() {
                                pixels[idx] = color;
                            }
                        }
                    }
                }
            }
            x += bw;
        }
        y += bh;
        row += 1;
    }

    // Vignette: a hard-ish falloff toward the corners, still deterministic.
    let inv_w = 1.0 / (w - 1) as f32;
    let inv_h = 1.0 / (h - 1) as f32;
    for py in 0..h {
        let fy = py as f32 * inv_h;
        for px in 0..w {
            let fx = px as f32 * inv_w;
            let dx = (fx - 0.5) * 1.8;
            let dy = fy - 0.5;
            let edge = (dx * dx + dy * dy).sqrt().clamp(0.0, 1.2);
            let darken = (edge * edge * 0.36).min(0.55);
            let idx = py * w + px;
            let p = pixels[idx];
            let hash = (px as u32).wrapping_mul(73_856_093) ^ (py as u32).wrapping_mul(19_349_663);
            let grain = ((hash >> 27) as i16) - 8;
            pixels[idx] = Color32::from_rgb(
                ((p.r() as f32 * (1.0 - darken) + grain as f32 * 0.35).clamp(0.0, 255.0)) as u8,
                ((p.g() as f32 * (1.0 - darken) + grain as f32 * 0.35).clamp(0.0, 255.0)) as u8,
                ((p.b() as f32 * (1.0 - darken) + grain as f32 * 0.35).clamp(0.0, 255.0)) as u8,
            );
        }
    }

    ColorImage {
        size: [w, h],
        source_size: Vec2::new(w as f32, h as f32),
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_deepslate_without_panicking() {
        let window = render(160, 96, Scene::Window);
        assert_eq!(window.size, [160, 96]);
        assert_eq!(window.pixels.len(), 160 * 96);
        assert_eq!(window.pixels[0].a(), 255);
        assert!(window.pixels[40 * 160 + 80].r() > 0);
        let hero = render(80, 48, Scene::Hero);
        assert_eq!(hero.pixels.len(), 80 * 48);
        let full = render(868, 704, Scene::Window);
        let max = full.pixels.iter().map(|p| p.r()).max().unwrap();
        assert!(max < 60, "backdrop must stay deepslate-dark: {max}");
    }
}
