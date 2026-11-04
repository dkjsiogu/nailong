//! 启动台的截图墙：读当前版本游戏目录里的 `screenshots/`，最新的在前，像商店游戏页下面那排截图。
//!
//! - 左键：大图预览，从缩略图的位置放大出来；←/→ 翻页，Esc 或点空白处关闭。
//! - 右键：把整张图片（不是路径）复制到剪贴板，缩略图上给一个「已复制」的反馈。
//! - 缩略图在后台解码、缩小，缓存到 `cache/cairn/thumbs/`，下次秒出；不卡界面。
//! - 只有动画进行中或有图在加载时才请求重绘，空闲 CPU 仍为 0；尊重「减少动效」。

use super::*;
use std::hash::{Hash, Hasher};

const THUMB_W: u32 = 320;
const THUMB_H: u32 = 180;
/// 截图目录最多这么久重新列一次：游戏运行中新截的图回到窗口就能看到。
const RESCAN_SECS: f64 = 3.0;
const OPEN_SECS: f32 = 0.2;

#[derive(Clone)]
pub(crate) struct Shot {
    pub path: PathBuf,
    pub modified: u64,
    size: u64,
}

enum Thumb {
    Loading,
    Ready(egui::TextureHandle, f64),
    Failed,
}

enum Job {
    Thumb(Shot),
    Full(PathBuf, Purpose),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    View,
    Copy,
}

enum Done {
    Thumb(PathBuf, Option<egui::ColorImage>),
    Full(PathBuf, Purpose, Option<egui::ColorImage>),
}

struct Viewer {
    index: usize,
    opened_at: f64,
    /// 从哪个缩略图放大出来。
    from: egui::Rect,
    closing_at: Option<f64>,
    full: Option<(PathBuf, egui::TextureHandle)>,
    copied_at: Option<f64>,
}

pub(crate) struct Shots {
    dir: PathBuf,
    checked_at: f64,
    list: Vec<Shot>,
    thumbs: HashMap<PathBuf, Thumb>,
    jobs: mpsc::Sender<Job>,
    done: mpsc::Receiver<Done>,
    viewer: Option<Viewer>,
    /// 右键复制：正在解码的、以及刚复制好的（用来画反馈）。
    copying: Option<PathBuf>,
    copied: Option<(PathBuf, f64)>,
}

fn decode(path: &std::path::Path) -> Option<image::RgbaImage> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
        .map(|image| image.into_rgba8())
}

fn color_image(image: &image::RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

fn thumb_path(cache: &std::path::Path, shot: &Shot) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    shot.path.hash(&mut hasher);
    shot.modified.hash(&mut hasher);
    shot.size.hash(&mut hasher);
    cache.join(format!("{:016x}.png", hasher.finish()))
}

/// 缩略图：有缓存读缓存，没有就从原图缩小再存一份。
fn make_thumb(cache: &std::path::Path, shot: &Shot) -> Option<egui::ColorImage> {
    let cached = thumb_path(cache, shot);
    if let Some(image) = decode(&cached) {
        return Some(color_image(&image));
    }
    let full = image::DynamicImage::ImageRgba8(decode(&shot.path)?);
    let small = full.thumbnail(THUMB_W, THUMB_H).into_rgba8();
    if std::fs::create_dir_all(cache).is_ok() {
        let _ = small.save(&cached);
    }
    Some(color_image(&small))
}

fn worker(ctx: egui::Context, cache: PathBuf, jobs: mpsc::Receiver<Job>, done: mpsc::Sender<Done>) {
    for job in jobs {
        let result = match job {
            Job::Thumb(shot) => Done::Thumb(shot.path.clone(), make_thumb(&cache, &shot)),
            Job::Full(path, purpose) => {
                let image = decode(&path).map(|image| color_image(&image));
                Done::Full(path, purpose, image)
            }
        };
        if done.send(result).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

fn list_shots(dir: &std::path::Path) -> Vec<Shot> {
    let mut shots: Vec<Shot> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some(Shot {
                path,
                modified,
                size: meta.len(),
            })
        })
        .collect();
    shots.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.path.cmp(&a.path)));
    shots
}

fn ease(t: f32) -> f32 {
    motion::ease_out_cubic(t.clamp(0.0, 1.0))
}

impl Shots {
    pub(crate) fn new(ctx: &egui::Context, dir: PathBuf, cache: PathBuf) -> Self {
        let (jobs, job_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker_ctx = ctx.clone();
        std::thread::spawn(move || worker(worker_ctx, cache, job_rx, done_tx));
        Self {
            dir,
            checked_at: f64::NEG_INFINITY,
            list: Vec::new(),
            thumbs: HashMap::new(),
            jobs,
            done,
            viewer: None,
            copying: None,
            copied: None,
        }
    }

    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// 收后台的结果、按需重新列目录。
    fn sync(&mut self, ctx: &egui::Context, now: f64) {
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Thumb(path, image) => {
                    let thumb = match image {
                        Some(image) => Thumb::Ready(
                            ctx.load_texture(
                                format!("shot-thumb:{}", path.display()),
                                image,
                                egui::TextureOptions::LINEAR,
                            ),
                            now,
                        ),
                        None => Thumb::Failed,
                    };
                    self.thumbs.insert(path, thumb);
                }
                Done::Full(path, Purpose::Copy, image) => {
                    if self.copying.as_ref() == Some(&path) {
                        self.copying = None;
                        if let Some(image) = image {
                            ctx.copy_image(image);
                            self.copied = Some((path.clone(), now));
                            if let Some(viewer) = &mut self.viewer {
                                viewer.copied_at = Some(now);
                            }
                            ctx.request_repaint_after(Duration::from_millis(1600));
                        }
                    }
                }
                Done::Full(path, Purpose::View, image) => {
                    let current = self
                        .viewer
                        .as_ref()
                        .and_then(|viewer| self.list.get(viewer.index))
                        .map(|shot| shot.path.clone());
                    if current.as_ref() == Some(&path)
                        && let (Some(viewer), Some(image)) = (&mut self.viewer, image)
                    {
                        let texture =
                            ctx.load_texture("shot-full", image, egui::TextureOptions::LINEAR);
                        viewer.full = Some((path, texture));
                    }
                }
            }
        }
        if now - self.checked_at > RESCAN_SECS {
            self.checked_at = now;
            let list = list_shots(&self.dir);
            let alive: HashSet<&PathBuf> = list.iter().map(|shot| &shot.path).collect();
            self.thumbs.retain(|path, _| alive.contains(path));
            self.list = list;
        }
    }

    fn request_thumb(&mut self, shot: &Shot) {
        if !self.thumbs.contains_key(&shot.path) {
            self.thumbs.insert(shot.path.clone(), Thumb::Loading);
            let _ = self.jobs.send(Job::Thumb(shot.clone()));
        }
    }

    fn copy(&mut self, path: PathBuf) {
        self.copying = Some(path.clone());
        self.copied = None;
        let _ = self.jobs.send(Job::Full(path, Purpose::Copy));
    }

    fn open_viewer(&mut self, index: usize, from: egui::Rect, now: f64) {
        self.viewer = Some(Viewer {
            index,
            opened_at: now,
            from,
            closing_at: None,
            full: None,
            copied_at: None,
        });
        if let Some(shot) = self.list.get(index) {
            let _ = self.jobs.send(Job::Full(shot.path.clone(), Purpose::View));
        }
    }

    fn show_index(&mut self, index: usize) {
        let Some(viewer) = &mut self.viewer else {
            return;
        };
        if index == viewer.index || index >= self.list.len() {
            return;
        }
        viewer.index = index;
        viewer.full = None;
        viewer.copied_at = None;
        let path = self.list[index].path.clone();
        if let Some(shot) = self.list.get(index).cloned() {
            self.request_thumb(&shot);
        }
        let _ = self.jobs.send(Job::Full(path, Purpose::View));
    }

    /// 启动台上的那一排。`width` 是内容宽度。
    pub(crate) fn strip(&mut self, ui: &mut egui::Ui, width: f32) -> bool {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        self.sync(&ctx, now);
        let mut open_folder_clicked = false;
        let total = self.list.len();

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("截图")
                    .size(13.0)
                    .family(bold_family())
                    .color(t::sub()),
            );
            if total > 0 {
                ui.label(
                    egui::RichText::new(format!("{total} 张"))
                        .size(13.0)
                        .color(t::dim()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action_link(ui, "打开文件夹").clicked() {
                    open_folder_clicked = true;
                }
                if total > 0 {
                    ui.label(
                        egui::RichText::new("左键查看 · 右键复制图片")
                            .size(12.0)
                            .color(t::dim()),
                    );
                }
            });
        });
        ui.add_space(6.0);
        if total == 0 {
            ui.label(
                egui::RichText::new("还没有截图 · 在游戏里按 F2，截的图会出现在这里")
                    .size(13.0)
                    .color(t::dim()),
            );
            return open_folder_clicked;
        }

        let gap = 10.0;
        let tile_w = 136.0;
        let tile_h = tile_w * 9.0 / 16.0;
        let fit = (((width + gap) / (tile_w + gap)).floor() as usize).max(1);
        let shown = total.min(fit);
        let tile_w = (width - gap * (shown as f32 - 1.0)) / fit as f32;
        let tile_w = tile_w.min(176.0);
        let tile_h = tile_h.max(tile_w * 9.0 / 16.0);
        let (strip, _) =
            ui.allocate_exact_size(egui::vec2(width, tile_h + 8.0), egui::Sense::hover());
        let mut animating = false;
        let mut open: Option<(usize, egui::Rect)> = None;
        let mut copy: Option<PathBuf> = None;
        let reduced = motion::reduced();
        for index in 0..shown {
            let shot = self.list[index].clone();
            self.request_thumb(&shot);
            let base = egui::Rect::from_min_size(
                egui::pos2(
                    strip.left() + index as f32 * (tile_w + gap),
                    strip.top() + 4.0,
                ),
                egui::vec2(tile_w, tile_h),
            );
            let id = egui::Id::unique(("shot", &shot.path));
            let response = ui.interact(base, id, egui::Sense::click());
            let hover = if reduced {
                if response.hovered() { 1.0 } else { 0.0 }
            } else {
                ctx.animate_bool_with_time(id, response.hovered(), 0.12)
            };
            if hover > 0.0 && hover < 1.0 {
                animating = true;
            }
            // 悬停时略微放大，像被拿起来一点。
            let rect = base.expand(hover * 3.0);
            let painter = ui.painter();
            painter.rect_filled(rect, motion::R, t::wash());
            match self.thumbs.get(&shot.path) {
                Some(Thumb::Ready(texture, loaded_at)) => {
                    let fade = if reduced {
                        1.0
                    } else {
                        ((now - loaded_at) / 0.18).clamp(0.0, 1.0) as f32
                    };
                    if fade < 1.0 {
                        animating = true;
                    }
                    let uv = cover_uv(texture.size_vec2(), rect.size());
                    painter.add(
                        egui::epaint::RectShape::filled(rect, motion::R, egui::Color32::WHITE)
                            .with_texture(texture.id(), uv),
                    );
                    if fade < 1.0 {
                        painter.rect_filled(rect, motion::R, t::tint(t::wash(), 1.0 - fade));
                    }
                }
                Some(Thumb::Failed) => {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "读不出",
                        egui::FontId::proportional(12.0),
                        t::dim(),
                    );
                }
                _ => {}
            }
            let border = motion::mix(t::hairline(), t::accent(), hover);
            painter.rect_stroke(
                rect,
                motion::R,
                egui::Stroke::new(1.0, border),
                egui::StrokeKind::Inside,
            );
            // 最后一格还有更多时盖一层「+N」。
            if index + 1 == shown && total > shown {
                painter.rect_filled(rect, motion::R, t::tint(egui::Color32::BLACK, 0.55));
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("+{}", total - shown),
                    egui::FontId::new(20.0, bold_family()),
                    egui::Color32::WHITE,
                );
            }
            // 右键复制的反馈：解码中一个转圈，复制好一个对勾，1.5 秒后淡出。
            if self.copying.as_ref() == Some(&shot.path) {
                badge(ui, rect, "复制中…", 1.0);
                animating = true;
            } else if let Some((path, at)) = &self.copied
                && path == &shot.path
            {
                let age = (now - at) as f32;
                if age < 1.5 {
                    badge(ui, rect, "✓ 已复制", (1.5 - age).min(0.3) / 0.3);
                    if age > 1.2 {
                        animating = true;
                    }
                }
            }
            if response.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let when = human_age(shot.modified);
            let name = shot
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let response =
                response.on_hover_text(format!("{name} · {when}\n左键查看 · 右键复制图片"));
            if response.clicked() {
                open = Some((index, rect));
            }
            if response.secondary_clicked() {
                copy = Some(shot.path.clone());
            }
        }
        if animating {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if let Some(path) = copy {
            self.copy(path);
            ctx.request_repaint();
        }
        if let Some((index, rect)) = open {
            self.open_viewer(index, rect, now);
            ctx.request_repaint();
        }
        open_folder_clicked
    }

    /// 大图预览，盖在整个窗口上。
    pub(crate) fn viewer(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.sync(ctx, now);
        let Some(viewer) = &self.viewer else {
            return;
        };
        let reduced = motion::reduced();
        let screen = ctx.content_rect();
        let open_t = if reduced {
            1.0
        } else {
            ease(((now - viewer.opened_at) as f32) / OPEN_SECS)
        };
        let close_t = match viewer.closing_at {
            Some(at) if reduced => {
                let _ = at;
                1.0
            }
            Some(at) => ((now - at) as f32 / 0.14).clamp(0.0, 1.0),
            None => 0.0,
        };
        if close_t >= 1.0 {
            self.viewer = None;
            return;
        }
        if open_t < 1.0 || viewer.closing_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let index = viewer.index;
        let Some(shot) = self.list.get(index).cloned() else {
            self.viewer = None;
            return;
        };
        let texture = viewer
            .full
            .as_ref()
            .filter(|(path, _)| path == &shot.path)
            .map(|(_, texture)| texture.clone())
            .or_else(|| match self.thumbs.get(&shot.path) {
                Some(Thumb::Ready(texture, _)) => Some(texture.clone()),
                _ => None,
            });
        let from = viewer.from;
        let copied_at = viewer.copied_at;
        let full_ready = viewer.full.is_some();
        let alpha = open_t * (1.0 - close_t);

        let mut close = false;
        let mut step: i32 = 0;
        let mut copy = false;
        let mut reveal = false;
        let mut external = false;
        egui::Area::new(egui::Id::unique("shot_viewer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (backdrop, response) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    backdrop,
                    0.0,
                    t::tint(egui::Color32::from_rgb(10, 11, 13), 0.92 * alpha),
                );
                if response.clicked() {
                    close = true;
                }
                // 图片放在中间，四周留边，底部留一条工具栏。
                let area = egui::Rect::from_min_max(
                    screen.min + egui::vec2(64.0, 40.0),
                    screen.max - egui::vec2(64.0, 76.0),
                );
                let size = texture
                    .as_ref()
                    .map(|t| t.size_vec2())
                    .unwrap_or(egui::vec2(16.0, 9.0));
                let scale = (area.width() / size.x).min(area.height() / size.y);
                let target = egui::Rect::from_center_size(area.center(), size * scale);
                let rect = lerp_rect(from, target, open_t);
                let rect = egui::Rect::from_center_size(
                    rect.center(),
                    rect.size() * (1.0 - 0.04 * close_t),
                );
                let image_response =
                    ui.interact(rect, egui::Id::unique("shot_big"), egui::Sense::click());
                if let Some(texture) = &texture {
                    ui.painter().add(
                        egui::epaint::RectShape::filled(
                            rect,
                            4,
                            egui::Color32::WHITE.gamma_multiply(alpha),
                        )
                        .with_texture(
                            texture.id(),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        ),
                    );
                } else {
                    ui.painter().rect_filled(rect, 4, t::tint(t::wash(), alpha));
                }
                if image_response.secondary_clicked() {
                    copy = true;
                }
                if !full_ready {
                    ui.painter().text(
                        rect.right_bottom() - egui::vec2(10.0, 10.0),
                        egui::Align2::RIGHT_BOTTOM,
                        "加载原图…",
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_white_alpha((180.0 * alpha) as u8),
                    );
                }
                // 两侧翻页。
                let side =
                    |ui: &mut egui::Ui, at: egui::Pos2, glyph: &str, enabled: bool| -> bool {
                        let rect = egui::Rect::from_center_size(at, egui::vec2(40.0, 64.0));
                        let response = ui.interact(
                            rect,
                            egui::Id::unique(("shot_side", glyph)),
                            egui::Sense::click(),
                        );
                        let shade = if response.hovered() && enabled {
                            0.9
                        } else {
                            0.45
                        };
                        if enabled {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                glyph,
                                egui::FontId::proportional(30.0),
                                egui::Color32::from_white_alpha((255.0 * shade * alpha) as u8),
                            );
                        }
                        enabled && response.clicked()
                    };
                if side(
                    ui,
                    egui::pos2(screen.left() + 32.0, area.center().y),
                    "‹",
                    index > 0,
                ) {
                    step = -1;
                }
                if side(
                    ui,
                    egui::pos2(screen.right() - 32.0, area.center().y),
                    "›",
                    index + 1 < self.list.len(),
                ) {
                    step = 1;
                }
                // 底部工具栏。
                let bar = egui::Rect::from_min_max(
                    egui::pos2(screen.left() + 64.0, screen.bottom() - 60.0),
                    egui::pos2(screen.right() - 64.0, screen.bottom() - 24.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                child.multiply_opacity(alpha);
                let name = shot
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                child.label(
                    egui::RichText::new(format!(
                        "{} / {} · {} · {}",
                        index + 1,
                        self.list.len(),
                        name,
                        human_age(shot.modified)
                    ))
                    .size(13.0)
                    .color(egui::Color32::from_gray(200)),
                );
                child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let light = |text: &str| {
                        egui::Button::new(
                            egui::RichText::new(text)
                                .size(13.0)
                                .color(egui::Color32::from_gray(225)),
                        )
                        .fill(egui::Color32::from_white_alpha(18))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)))
                        .corner_radius(motion::R)
                        .min_size(egui::vec2(0.0, 30.0))
                    };
                    if ui.add(light("关闭")).on_hover_text("Esc").clicked() {
                        close = true;
                    }
                    if ui.add(light("用看图软件打开")).clicked() {
                        external = true;
                    }
                    if ui.add(light("在文件夹中显示")).clicked() {
                        reveal = true;
                    }
                    let copied = copied_at.is_some_and(|at| now - at < 1.5);
                    let copying = self.copying.as_ref() == Some(&shot.path);
                    let label = if copied {
                        "✓ 已复制"
                    } else if copying {
                        "复制中…"
                    } else {
                        "复制图片"
                    };
                    if ui
                        .add(light(label))
                        .on_hover_text("Ctrl+C，或在图上点右键")
                        .clicked()
                    {
                        copy = true;
                    }
                });
            });
        let keys = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.modifiers.command && i.key_pressed(egui::Key::C),
            )
        });
        if keys.0 {
            close = true;
        }
        if keys.1 && index > 0 {
            step = -1;
        }
        if keys.2 && index + 1 < self.list.len() {
            step = 1;
        }
        if keys.3 {
            copy = true;
        }
        if copy && self.copying.is_none() {
            self.copy(shot.path.clone());
        }
        if reveal {
            reveal_file(&shot.path);
        }
        if external {
            open_folder(&shot.path);
        }
        if step != 0 {
            self.show_index((index as i32 + step) as usize);
            ctx.request_repaint();
        }
        if close && let Some(viewer) = &mut self.viewer {
            if viewer.closing_at.is_none() {
                viewer.closing_at = Some(now);
            }
            ctx.request_repaint();
        }
    }

    pub(crate) fn viewing(&self) -> bool {
        self.viewer.is_some()
    }
}

/// 让缩略图铺满格子（多出来的裁掉），不留黑边。
fn cover_uv(texture: egui::Vec2, cell: egui::Vec2) -> egui::Rect {
    let texture_ratio = texture.x / texture.y.max(1.0);
    let cell_ratio = cell.x / cell.y.max(1.0);
    if texture_ratio > cell_ratio {
        let w = cell_ratio / texture_ratio;
        egui::Rect::from_min_max(
            egui::pos2((1.0 - w) / 2.0, 0.0),
            egui::pos2((1.0 + w) / 2.0, 1.0),
        )
    } else {
        let h = texture_ratio / cell_ratio;
        egui::Rect::from_min_max(
            egui::pos2(0.0, (1.0 - h) / 2.0),
            egui::pos2(1.0, (1.0 + h) / 2.0),
        )
    }
}

fn lerp_rect(from: egui::Rect, to: egui::Rect, t: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        from.min + (to.min - from.min) * t,
        from.max + (to.max - from.max) * t,
    )
}

/// 缩略图底部的一条小提示。
fn badge(ui: &egui::Ui, rect: egui::Rect, text: &str, alpha: f32) {
    let band = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - 24.0),
        rect.right_bottom(),
    );
    ui.painter().rect_filled(
        band,
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: motion::R,
            se: motion::R,
        },
        t::tint(egui::Color32::BLACK, 0.62 * alpha),
    );
    ui.painter().text(
        band.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(12.5),
        egui::Color32::from_white_alpha((235.0 * alpha) as u8),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_long_side() {
        let uv = cover_uv(egui::vec2(1920.0, 1080.0), egui::vec2(100.0, 100.0));
        assert!((uv.width() - 0.5625).abs() < 1e-3);
        assert!((uv.height() - 1.0).abs() < 1e-6);
        let uv = cover_uv(egui::vec2(1080.0, 1920.0), egui::vec2(160.0, 90.0));
        assert!((uv.width() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lists_newest_images_and_caches_thumbnails() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("cairn-shots-{}-{n}", std::process::id()));
        let dir = base.join("screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        let image = image::RgbaImage::from_pixel(640, 360, image::Rgba([90, 120, 200, 255]));
        image.save(dir.join("a.png")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        image.save(dir.join("b.png")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let shots = list_shots(&dir);
        assert_eq!(shots.len(), 2);
        assert!(shots[0].path.ends_with("b.png"));
        let cache = base.join("thumbs");
        let thumb = make_thumb(&cache, &shots[0]).unwrap();
        assert_eq!(thumb.size, [320, 180]);
        assert!(thumb_path(&cache, &shots[0]).is_file());
        // 第二次直接读缓存。
        assert_eq!(make_thumb(&cache, &shots[0]).unwrap().size, [320, 180]);
        let _ = std::fs::remove_dir_all(&base);
    }
}
