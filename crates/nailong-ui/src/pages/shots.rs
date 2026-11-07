//! 截图的底座：全部版本的截图列表、后台缩略图、启动台那一排，以及看大图。
//! 截图页（[`super::gallery`]）和启动台共用这一份，点开的图都能缩放、翻页、看到时间地点人物。
//!
//! - 列表来自 `core::shots::scan`：时间读文件名，地点和人物读图片里 Cairn 写的信息，
//!   没有就用从日志推断的。在后台列，不卡界面。
//! - 缩略图在后台解码、缩小，缓存到 `cache/cairn/thumbs/`，下次秒出。
//! - 看大图：滚轮缩放（以鼠标为中心）、双击在「适应窗口」和「原尺寸」之间切换、拖动平移、
//!   ←/→ 在当前这一组里翻页、底部一排缩略图；能复制、在文件夹中显示、设为世界图标、移到回收站。
//! - 只有动画进行中或有图在加载时才请求重绘，空闲 CPU 仍为 0；尊重「减少动效」。

use crate::prelude::*;
use chrono::{Datelike, NaiveDateTime};
use nailong_core::shots::Meta;
use std::hash::{Hash, Hasher};

const THUMB_W: u32 = 320;
const THUMB_H: u32 = 180;
const OPEN_SECS: f32 = 0.2;

/// 一张截图，带着界面要用的标签。
#[derive(Clone, Debug)]
pub(crate) struct Shot {
    pub(crate) path: PathBuf,
    pub(crate) game_dir: PathBuf,
    pub(crate) time: NaiveDateTime,
    pub(crate) size: u64,
    pub(crate) meta: Meta,
    /// 给人看的版本名：图片里记着的，或者这个游戏目录对应的版本。
    pub(crate) version: String,
}

enum Thumb {
    Loading,
    Ready(egui::TextureHandle, f64),
    Failed,
}

enum Job {
    Thumb(PathBuf, u64),
    Full(PathBuf, Purpose),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    View,
    Copy,
    Icon,
}

enum Done {
    Thumb(PathBuf, Option<egui::ColorImage>),
    Full(PathBuf, Purpose, Option<image::RgbaImage>),
    Scanned(Vec<Shot>),
}

struct Viewer {
    /// 这一组（时间段、相簿、人物……）里的截图，按显示顺序。
    seq: Vec<PathBuf>,
    pos: usize,
    opened_at: f64,
    /// 从哪个缩略图放大出来。
    from: egui::Rect,
    closing_at: Option<f64>,
    full: Option<(PathBuf, egui::TextureHandle, [usize; 2])>,
    copied_at: Option<f64>,
    /// 1.0 是适应窗口。
    zoom: f32,
    pan: egui::Vec2,
    /// 「移到回收站」点了一次，等确认。
    trash_armed: bool,
}

/// 看大图时做完的事，交回给 App 处理（要写提示、刷新列表）。
pub(crate) enum ViewerEvent {
    Trashed(PathBuf),
    TrashFailed(String),
    IconSet(String),
    IconFailed(String),
}

pub(crate) struct Shots {
    root: Root,
    /// 全部截图，最新的在前。
    pub(crate) list: Vec<Shot>,
    /// 至少列完一次。
    pub(crate) loaded: bool,
    scanning: bool,
    scanned_at: f64,
    thumbs: HashMap<PathBuf, Thumb>,
    jobs: mpsc::Sender<Job>,
    done_tx: mpsc::Sender<Done>,
    done: mpsc::Receiver<Done>,
    viewer: Option<Viewer>,
    /// 右键复制：正在解码的、以及刚复制好的（用来画反馈）。
    copying: Option<PathBuf>,
    copied: Option<(PathBuf, f64)>,
    /// 要设为图标的：图片路径、世界文件夹。
    icon_for: Option<(PathBuf, PathBuf)>,
    events: Vec<ViewerEvent>,
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

fn thumb_path(cache: &std::path::Path, path: &std::path::Path, size: u64) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    size.hash(&mut hasher);
    cache.join(format!("{:016x}.png", hasher.finish()))
}

/// 缩略图：有缓存读缓存，没有就从原图缩小再存一份。
/// 只按路径和大小认缓存：写进时间地点人物会改文件，但不改大小以外的东西……
/// 其实大小也会变（多了一小块文字），那就重新缩一次，不碍事。
fn make_thumb(
    cache: &std::path::Path,
    path: &std::path::Path,
    size: u64,
) -> Option<egui::ColorImage> {
    let cached = thumb_path(cache, path, size);
    if let Some(image) = decode(&cached) {
        return Some(color_image(&image));
    }
    let full = image::DynamicImage::ImageRgba8(decode(path)?);
    let small = full.thumbnail(THUMB_W, THUMB_H).into_rgba8();
    if std::fs::create_dir_all(cache).is_ok() {
        let _ = small.save(&cached);
    }
    Some(color_image(&small))
}

fn worker(ctx: egui::Context, cache: PathBuf, jobs: mpsc::Receiver<Job>, done: mpsc::Sender<Done>) {
    for job in jobs {
        let result = match job {
            Job::Thumb(path, size) => {
                let image = make_thumb(&cache, &path, size);
                Done::Thumb(path, image)
            }
            Job::Full(path, purpose) => {
                let image = decode(&path);
                Done::Full(path, purpose, image)
            }
        };
        if done.send(result).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

fn ease(t: f32) -> f32 {
    motion::ease_out_cubic(t.clamp(0.0, 1.0))
}

/// 「11月7日 周六 21:34」；不是今年的带上年份。
pub(crate) fn when_label(time: &NaiveDateTime) -> String {
    let today = chrono::Local::now().date_naive();
    let date = time.date();
    let day = if date == today {
        "今天".to_string()
    } else if Some(date) == today.pred_opt() {
        "昨天".to_string()
    } else if date.year() == today.year() {
        format!("{}月{}日 {}", date.month(), date.day(), weekday(date))
    } else {
        format!("{}年{}月{}日", date.year(), date.month(), date.day())
    };
    format!("{day} {}", time.format("%H:%M"))
}

pub(crate) fn weekday(date: chrono::NaiveDate) -> &'static str {
    ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]
        [date.weekday().num_days_from_monday() as usize]
}

impl Shots {
    pub(crate) fn new(ctx: &egui::Context, root: Root) -> Self {
        let (jobs, job_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker_ctx = ctx.clone();
        let cache = root.cache().join("thumbs");
        let worker_done = done_tx.clone();
        std::thread::spawn(move || worker(worker_ctx, cache, job_rx, worker_done));
        Self {
            root,
            list: Vec::new(),
            loaded: false,
            scanning: false,
            scanned_at: f64::NEG_INFINITY,
            thumbs: HashMap::new(),
            jobs,
            done_tx,
            done,
            viewer: None,
            copying: None,
            copied: None,
            icon_for: None,
            events: Vec::new(),
        }
    }

    /// 在后台重新列一遍。`dirs` 是每个游戏目录和它对应的版本名。
    pub(crate) fn rescan(&mut self, ctx: &egui::Context, dirs: Vec<(PathBuf, String)>) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        self.scanned_at = ctx.input(|i| i.time);
        let root = self.root.clone();
        let tx = self.done_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let paths: Vec<PathBuf> = dirs.iter().map(|(dir, _)| dir.clone()).collect();
            let labels: HashMap<PathBuf, String> = dirs.into_iter().collect();
            let list = nailong_core::shots::scan(&root, &paths)
                .into_iter()
                .map(|shot| Shot {
                    version: shot
                        .meta
                        .version
                        .clone()
                        .or_else(|| labels.get(&shot.game_dir).cloned())
                        .unwrap_or_else(|| "未知版本".into()),
                    path: shot.path,
                    game_dir: shot.game_dir,
                    time: shot.time,
                    size: shot.size,
                    meta: shot.meta,
                })
                .collect();
            let _ = tx.send(Done::Scanned(list));
            ctx.request_repaint();
        });
    }

    /// 上次列目录是多久以前（秒）。
    pub(crate) fn age(&self, now: f64) -> f64 {
        now - self.scanned_at
    }

    pub(crate) fn take_events(&mut self) -> Vec<ViewerEvent> {
        std::mem::take(&mut self.events)
    }

    /// 收后台的结果。
    pub(crate) fn sync(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Scanned(list) => {
                    self.scanning = false;
                    self.loaded = true;
                    let alive: HashSet<&PathBuf> = list.iter().map(|shot| &shot.path).collect();
                    self.thumbs.retain(|path, _| alive.contains(path));
                    // 文件大小变了（刚写进时间地点人物）的缩略图要重新取，免得认错缓存。
                    for shot in &list {
                        if let Some(old) = self.list.iter().find(|s| s.path == shot.path)
                            && old.size != shot.size
                        {
                            self.thumbs.remove(&shot.path);
                        }
                    }
                    self.list = list;
                }
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
                            ctx.copy_image(color_image(&image));
                            self.copied = Some((path.clone(), now));
                            if let Some(viewer) = &mut self.viewer {
                                viewer.copied_at = Some(now);
                            }
                            ctx.request_repaint_after(Duration::from_millis(1600));
                        }
                    }
                }
                Done::Full(path, Purpose::Icon, image) => {
                    if let Some((wanted, world)) = self.icon_for.take()
                        && wanted == path
                    {
                        let name = world
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        match image.and_then(|image| world_icon(&image, &world)) {
                            Some(()) => self.events.push(ViewerEvent::IconSet(name)),
                            None => self
                                .events
                                .push(ViewerEvent::IconFailed(format!("无法写入 {name} 的图标"))),
                        }
                    }
                }
                Done::Full(path, Purpose::View, image) => {
                    let current = self
                        .viewer
                        .as_ref()
                        .and_then(|viewer| viewer.seq.get(viewer.pos))
                        .cloned();
                    if current.as_ref() == Some(&path)
                        && let (Some(viewer), Some(image)) = (&mut self.viewer, image)
                    {
                        let size = [image.width() as usize, image.height() as usize];
                        let texture = ctx.load_texture(
                            "shot-full",
                            color_image(&image),
                            egui::TextureOptions::LINEAR,
                        );
                        viewer.full = Some((path, texture, size));
                    }
                }
            }
        }
    }

    pub(crate) fn get(&self, path: &std::path::Path) -> Option<&Shot> {
        self.list.iter().find(|shot| shot.path == path)
    }

    pub(crate) fn request_thumb(&mut self, shot: &Shot) {
        if !self.thumbs.contains_key(&shot.path) {
            self.thumbs.insert(shot.path.clone(), Thumb::Loading);
            let _ = self.jobs.send(Job::Thumb(shot.path.clone(), shot.size));
        }
    }

    pub(crate) fn copy(&mut self, path: PathBuf) {
        self.copying = Some(path.clone());
        self.copied = None;
        let _ = self.jobs.send(Job::Full(path, Purpose::Copy));
    }

    /// 打开大图：`seq` 是能翻页的这一组，`path` 是先看哪张。
    pub(crate) fn open_viewer(
        &mut self,
        seq: Vec<PathBuf>,
        path: &std::path::Path,
        from: egui::Rect,
        now: f64,
    ) {
        let pos = seq.iter().position(|p| p == path).unwrap_or(0);
        let Some(first) = seq.get(pos).cloned() else {
            return;
        };
        self.viewer = Some(Viewer {
            seq,
            pos,
            opened_at: now,
            from,
            closing_at: None,
            full: None,
            copied_at: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            trash_armed: false,
        });
        let _ = self.jobs.send(Job::Full(first, Purpose::View));
    }

    fn show_pos(&mut self, pos: usize) {
        let Some(viewer) = &mut self.viewer else {
            return;
        };
        if pos == viewer.pos || pos >= viewer.seq.len() {
            return;
        }
        viewer.pos = pos;
        viewer.full = None;
        viewer.copied_at = None;
        viewer.zoom = 1.0;
        viewer.pan = egui::Vec2::ZERO;
        viewer.trash_armed = false;
        let path = viewer.seq[pos].clone();
        if let Some(shot) = self.get(&path).cloned() {
            self.request_thumb(&shot);
        }
        let _ = self.jobs.send(Job::Full(path, Purpose::View));
    }

    pub(crate) fn viewing(&self) -> bool {
        self.viewer.is_some()
    }

    /// 画一格缩略图（悬停微微放大、淡入、复制反馈）。返回这一格的点击结果。
    /// `scope` 区分同一张图出现在几处（最近、相簿封面……），免得控件编号撞上。
    pub(crate) fn tile(
        &mut self,
        ui: &mut egui::Ui,
        scope: &str,
        shot: &Shot,
        base: egui::Rect,
        radius: u8,
        hover_text: Option<String>,
    ) -> egui::Response {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        self.request_thumb(shot);
        let id = egui::Id::unique(("shot", scope, &shot.path));
        let response = ui.interact(base, id, egui::Sense::click());
        let reduced = motion::reduced();
        let hover = if reduced {
            if response.hovered() { 1.0 } else { 0.0 }
        } else {
            ctx.animate_bool_with_time(id, response.hovered(), 0.12)
        };
        let rect = base.expand(hover * 2.0);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, t::wash());
        let mut animating = hover > 0.0 && hover < 1.0;
        match self.thumbs.get(&shot.path) {
            Some(Thumb::Ready(texture, loaded_at)) => {
                let fade = if reduced {
                    1.0
                } else {
                    ((now - loaded_at) / 0.18).clamp(0.0, 1.0) as f32
                };
                animating |= fade < 1.0;
                let uv = cover_uv(texture.size_vec2(), rect.size());
                painter.add(
                    egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE)
                        .with_texture(texture.id(), uv),
                );
                if fade < 1.0 {
                    painter.rect_filled(rect, radius, t::tint(t::wash(), 1.0 - fade));
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
        if hover > 0.0 {
            painter.rect_stroke(
                rect,
                radius,
                egui::Stroke::new(1.5, t::tint(t::accent(), hover)),
                egui::StrokeKind::Inside,
            );
        }
        if self.copying.as_ref() == Some(&shot.path) {
            badge(ui, rect, "复制中…", 1.0);
            animating = true;
        } else if let Some((path, at)) = &self.copied
            && path == &shot.path
        {
            let age = (now - at) as f32;
            if age < 1.5 {
                badge(ui, rect, "✓ 已复制", (1.5 - age).min(0.3) / 0.3);
                animating |= age > 1.2;
            }
        }
        if animating {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let response = match hover_text {
            Some(text) => response.on_hover_text(text),
            None => response,
        };
        if response.secondary_clicked() {
            self.copy(shot.path.clone());
            ctx.request_repaint();
        }
        response
    }

    /// 启动台上的那一排：只看这个游戏目录的。返回（打开文件夹，去截图页）。
    pub(crate) fn strip(
        &mut self,
        ui: &mut egui::Ui,
        game_dir: &std::path::Path,
        width: f32,
    ) -> (bool, bool) {
        let now = ui.input(|i| i.time);
        let mine: Vec<Shot> = self
            .list
            .iter()
            .filter(|shot| shot.game_dir == game_dir)
            .cloned()
            .collect();
        let total = mine.len();
        let mut open_folder_clicked = false;
        let mut open_gallery = false;

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let header = ui
                .add(
                    egui::Label::new(
                        egui::RichText::new("截图")
                            .size(13.0)
                            .family(bold_family())
                            .color(t::sub()),
                    )
                    .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("打开截图页");
            if header.clicked() {
                open_gallery = true;
            }
            if total > 0
                && ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(format!("{total} 张 ›"))
                                .size(13.0)
                                .color(t::dim()),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("查看全部")
                    .clicked()
            {
                open_gallery = true;
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
                egui::RichText::new(if self.loaded {
                    "还没有截图 · 游戏里按 F2 截图"
                } else {
                    "正在找截图…"
                })
                .size(13.0)
                .color(t::dim()),
            );
            return (open_folder_clicked, open_gallery);
        }

        let gap = 10.0;
        let tile_w = 136.0;
        let fit = (((width + gap) / (tile_w + gap)).floor() as usize).max(1);
        let shown = total.min(fit);
        let tile_w = ((width - gap * (fit as f32 - 1.0)) / fit as f32).min(176.0);
        let tile_h = tile_w * 9.0 / 16.0;
        let (strip, _) =
            ui.allocate_exact_size(egui::vec2(width, tile_h + 8.0), egui::Sense::hover());
        let seq: Vec<PathBuf> = mine.iter().map(|shot| shot.path.clone()).collect();
        let mut open: Option<(PathBuf, egui::Rect)> = None;
        for (index, shot) in mine.iter().take(shown).enumerate() {
            let base = egui::Rect::from_min_size(
                egui::pos2(
                    strip.left() + index as f32 * (tile_w + gap),
                    strip.top() + 4.0,
                ),
                egui::vec2(tile_w, tile_h),
            );
            let more = index + 1 == shown && total > shown;
            let hover = if more {
                format!("还有 {} 张", total - shown)
            } else {
                format!("{}\n左键查看 · 右键复制图片", place_line(shot))
            };
            let response = self.tile(ui, "strip", shot, base, motion::R, Some(hover));
            // 最后一格还有更多时盖一层「+N」，点它去截图页。
            if more {
                ui.painter()
                    .rect_filled(base, motion::R, t::tint(egui::Color32::BLACK, 0.55));
                ui.painter().text(
                    base.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("+{}", total - shown),
                    egui::FontId::new(20.0, bold_family()),
                    egui::Color32::WHITE,
                );
            }
            if response.clicked() {
                if more {
                    open_gallery = true;
                } else {
                    open = Some((shot.path.clone(), base));
                }
            }
        }
        if let Some((path, rect)) = open {
            self.open_viewer(seq, &path, rect, now);
            ui.ctx().request_repaint();
        }
        (open_folder_clicked, open_gallery)
    }

    /// 大图预览，盖在整个窗口上。
    pub(crate) fn viewer(
        &mut self,
        ctx: &egui::Context,
        faces: &HashMap<String, egui::TextureHandle>,
    ) {
        let now = ctx.input(|i| i.time);
        self.sync(ctx);
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
            Some(_) if reduced => 1.0,
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
        let pos = viewer.pos;
        let count = viewer.seq.len();
        let Some(shot) = viewer.seq.get(pos).and_then(|path| self.get(path)).cloned() else {
            self.viewer = None;
            return;
        };
        let texture = viewer
            .full
            .as_ref()
            .filter(|(path, _, _)| path == &shot.path)
            .map(|(_, texture, _)| texture.clone())
            .or_else(|| match self.thumbs.get(&shot.path) {
                Some(Thumb::Ready(texture, _)) => Some(texture.clone()),
                _ => None,
            });
        let resolution = viewer
            .full
            .as_ref()
            .filter(|(path, _, _)| path == &shot.path)
            .map(|(_, _, size)| *size);
        let from = viewer.from;
        let copied_at = viewer.copied_at;
        let mut zoom = viewer.zoom;
        let mut pan = viewer.pan;
        let trash_armed = viewer.trash_armed;
        let strip_paths: Vec<PathBuf> = {
            let start = pos.saturating_sub(4);
            let end = (start + 9).min(count);
            let start = end.saturating_sub(9);
            viewer.seq[start..end].to_vec()
        };
        let alpha = open_t * (1.0 - close_t);
        let world_dir = shot
            .meta
            .world
            .as_ref()
            .map(|w| shot.game_dir.join("saves").join(w))
            .filter(|dir| dir.join("level.dat").is_file());

        let mut close = false;
        let mut step: i32 = 0;
        let mut jump_path: Option<PathBuf> = None;
        let mut copy = false;
        let mut reveal = false;
        let mut set_icon = false;
        let mut trash = false;
        let mut arm_trash = None;
        let strip_shots: Vec<Shot> = strip_paths
            .iter()
            .filter_map(|p| self.get(p).cloned())
            .collect();
        for s in &strip_shots {
            self.request_thumb(s);
        }
        let strip_textures: Vec<Option<egui::TextureHandle>> = strip_shots
            .iter()
            .map(|s| match self.thumbs.get(&s.path) {
                Some(Thumb::Ready(texture, _)) => Some(texture.clone()),
                _ => None,
            })
            .collect();
        let copying = self.copying.as_ref() == Some(&shot.path);
        egui::Area::new(egui::Id::unique("shot_viewer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (backdrop, backdrop_response) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    backdrop,
                    0.0,
                    egui::Color32::from_rgb(12, 13, 16).gamma_multiply(alpha),
                );
                if backdrop_response.clicked() {
                    close = true;
                }
                let ink = egui::Color32::from_gray(228).gamma_multiply(alpha);
                let dim = egui::Color32::from_gray(150).gamma_multiply(alpha);
                // 顶栏：时间、地点人物版本、操作。
                let top = egui::Rect::from_min_max(
                    screen.min,
                    egui::pos2(screen.right(), screen.top() + 60.0),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(top.shrink2(egui::vec2(16.0, 10.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                bar.multiply_opacity(alpha);
                if bar
                    .add(
                        egui::Button::new(egui::RichText::new("×").size(22.0).color(ink))
                            .frame(false),
                    )
                    .on_hover_text("关闭（Esc）")
                    .clicked()
                {
                    close = true;
                }
                bar.add_space(8.0);
                let face = shot.meta.player.as_ref().and_then(|p| faces.get(p));
                bar.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        egui::RichText::new(when_label(&shot.time))
                            .size(15.0)
                            .color(ink),
                    );
                    ui.horizontal(|ui| {
                        if let Some(face) = face {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            draw_avatar(ui.painter(), rect, "", Some(face));
                        }
                        let mut parts = vec![place_line(&shot)];
                        if let Some(player) = &shot.meta.player {
                            parts.push(player.clone());
                        }
                        if let Some([w, h]) = resolution {
                            parts.push(format!("{w}×{h}"));
                        }
                        parts.push(human_size(shot.size));
                        ui.label(egui::RichText::new(parts.join(" · ")).size(12.5).color(dim))
                            .on_hover_text(shot.path.display().to_string());
                    });
                });
                bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let light = |text: &str, color: egui::Color32| {
                        egui::Button::new(egui::RichText::new(text).size(13.5).color(color))
                            .fill(egui::Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(motion::R)
                            .min_size(egui::vec2(0.0, 30.0))
                    };
                    let danger = egui::Color32::from_rgb(214, 150, 142).gamma_multiply(alpha);
                    let trash_label = if trash_armed {
                        "确认移到回收站"
                    } else {
                        "移到回收站"
                    };
                    if ui
                        .add(light(trash_label, danger))
                        .on_hover_text("Delete")
                        .clicked()
                    {
                        if trash_armed {
                            trash = true;
                        } else {
                            arm_trash = Some(true);
                        }
                    }
                    if world_dir.is_some()
                        && ui
                            .add(light("设为世界图标", ink))
                            .on_hover_text("用作存档列表里的世界图标")
                            .clicked()
                    {
                        set_icon = true;
                    }
                    if ui.add(light("在文件夹中显示", ink)).clicked() {
                        reveal = true;
                    }
                    let copied = copied_at.is_some_and(|at| now - at < 1.5);
                    let label = if copied {
                        "✓ 已复制"
                    } else if copying {
                        "复制中…"
                    } else {
                        "复制"
                    };
                    if ui
                        .add(light(label, ink))
                        .on_hover_text("Ctrl+C 或右键")
                        .clicked()
                    {
                        copy = true;
                    }
                });

                // 图片区：适应窗口，滚轮缩放，拖动平移。
                let area = egui::Rect::from_min_max(
                    screen.min + egui::vec2(56.0, 64.0),
                    screen.max - egui::vec2(56.0, 92.0),
                );
                let size = texture
                    .as_ref()
                    .map(|t| t.size_vec2())
                    .unwrap_or(egui::vec2(16.0, 9.0));
                let natural = resolution
                    .map(|[w, h]| egui::vec2(w as f32, h as f32))
                    .unwrap_or(size);
                let fit = (area.width() / natural.x).min(area.height() / natural.y);
                let target =
                    egui::Rect::from_center_size(area.center() + pan, natural * fit * zoom);
                let rect = if open_t < 1.0 {
                    lerp_rect(from, target, open_t)
                } else {
                    target
                };
                let rect = egui::Rect::from_center_size(
                    rect.center(),
                    rect.size() * (1.0 - 0.04 * close_t),
                );
                let image_response = ui.interact(
                    area,
                    egui::Id::unique("shot_big"),
                    egui::Sense::click_and_drag(),
                );
                // 放大后只在顶栏和底栏之间画，不盖住按钮和缩略图。
                let clip = egui::Rect::from_min_max(
                    egui::pos2(screen.left(), screen.top() + 60.0),
                    egui::pos2(screen.right(), screen.bottom() - 80.0),
                );
                let painter = ui.painter().with_clip_rect(clip);
                if let Some(texture) = &texture {
                    painter.add(
                        egui::epaint::RectShape::filled(
                            rect,
                            2,
                            egui::Color32::WHITE.gamma_multiply(alpha),
                        )
                        .with_texture(
                            texture.id(),
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        ),
                    );
                } else {
                    painter.rect_filled(rect, 2, t::tint(egui::Color32::from_gray(40), alpha));
                }
                if resolution.is_none() {
                    painter.text(
                        rect.right_bottom() - egui::vec2(10.0, 10.0),
                        egui::Align2::RIGHT_BOTTOM,
                        "加载原图…",
                        egui::FontId::proportional(12.0),
                        dim,
                    );
                }
                if image_response.hovered() {
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 {
                        let old = zoom;
                        zoom = (zoom * (1.0 + scroll * 0.0025)).clamp(1.0, 12.0);
                        if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                            // 以鼠标为中心缩放：鼠标下面那个点不动。
                            let center = area.center() + pan;
                            pan += (pointer - center) * (1.0 - zoom / old);
                        }
                    }
                }
                if image_response.double_clicked() {
                    if zoom > 1.01 {
                        zoom = 1.0;
                        pan = egui::Vec2::ZERO;
                    } else {
                        // 原尺寸：1 个图片像素对 1 个屏幕像素。
                        let ppp = ui.ctx().pixels_per_point();
                        zoom = (1.0 / (fit * ppp)).max(1.0);
                        if let Some(pointer) = image_response.interact_pointer_pos() {
                            pan = (area.center() - pointer) * (zoom - 1.0);
                        }
                    }
                } else if image_response.dragged() && zoom > 1.0 {
                    pan += image_response.drag_delta();
                } else if image_response.clicked()
                    && !rect.contains(image_response.interact_pointer_pos().unwrap_or_default())
                {
                    close = true;
                }
                if zoom <= 1.0 {
                    pan = egui::Vec2::ZERO;
                } else {
                    // 别把图拖出视野：至少留一半在里面。
                    let half = (natural * fit * zoom - area.size()).max(egui::Vec2::ZERO) * 0.5;
                    pan = pan.clamp(-half, half);
                }
                if image_response.secondary_clicked() {
                    copy = true;
                }
                if zoom > 1.0 && image_response.hovered() {
                    ui.ctx().set_cursor_icon(if image_response.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
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
                        if enabled {
                            let shade = if response.hovered() { 0.9 } else { 0.4 };
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
                    egui::pos2(screen.left() + 28.0, area.center().y),
                    "‹",
                    pos > 0,
                ) {
                    step = -1;
                }
                if side(
                    ui,
                    egui::pos2(screen.right() - 28.0, area.center().y),
                    "›",
                    pos + 1 < count,
                ) {
                    step = 1;
                }
                // 底部：提示、一排缩略图、第几张。
                let bottom = screen.bottom() - 46.0;
                ui.painter().text(
                    egui::pos2(screen.left() + 24.0, bottom),
                    egui::Align2::LEFT_CENTER,
                    if zoom > 1.01 {
                        format!(
                            "{:.0}% · 双击还原",
                            zoom * fit * ui.ctx().pixels_per_point() * 100.0
                        )
                    } else {
                        "滚轮缩放 · 双击原尺寸".to_string()
                    },
                    egui::FontId::proportional(12.5),
                    dim,
                );
                ui.painter().text(
                    egui::pos2(screen.right() - 24.0, bottom),
                    egui::Align2::RIGHT_CENTER,
                    format!("{} / {count}", pos + 1),
                    egui::FontId::proportional(12.5),
                    dim,
                );
                let thumb = egui::vec2(64.0, 36.0);
                let gap = 6.0;
                let total_w = strip_shots.len() as f32 * (thumb.x + gap) - gap + 12.0;
                let mut x = screen.center().x - total_w / 2.0;
                for (s, texture) in strip_shots.iter().zip(&strip_textures) {
                    let current = s.path == shot.path;
                    let size = if current {
                        egui::vec2(76.0, 43.0)
                    } else {
                        thumb
                    };
                    let r = egui::Rect::from_min_size(egui::pos2(x, bottom - size.y / 2.0), size);
                    x += size.x + gap;
                    let response = ui.interact(
                        r,
                        egui::Id::unique(("shot_film", &s.path)),
                        egui::Sense::click(),
                    );
                    let fade = if current || response.hovered() {
                        1.0
                    } else {
                        0.55
                    };
                    match texture {
                        Some(texture) => {
                            ui.painter().add(
                                egui::epaint::RectShape::filled(
                                    r,
                                    2,
                                    egui::Color32::WHITE.gamma_multiply(alpha * fade),
                                )
                                .with_texture(
                                    texture.id(),
                                    cover_uv(texture.size_vec2(), r.size()),
                                ),
                            );
                        }
                        None => {
                            ui.painter().rect_filled(r, 2, egui::Color32::from_gray(40));
                        }
                    }
                    if current {
                        ui.painter().rect_stroke(
                            r.expand(2.0),
                            3,
                            egui::Stroke::new(2.0, t::accent().gamma_multiply(alpha)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        jump_path = Some(s.path.clone());
                    }
                }
            });
        let keys = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.modifiers.command && i.key_pressed(egui::Key::C),
                i.key_pressed(egui::Key::Delete),
            )
        });
        if keys.0 {
            close = true;
        }
        if keys.1 && pos > 0 {
            step = -1;
        }
        if keys.2 && pos + 1 < count {
            step = 1;
        }
        if keys.3 {
            copy = true;
        }
        if keys.4 {
            if trash_armed {
                trash = true;
            } else {
                arm_trash = Some(true);
            }
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.zoom = zoom;
            viewer.pan = pan;
            if let Some(armed) = arm_trash {
                viewer.trash_armed = armed;
            }
        }
        if copy && self.copying.is_none() {
            self.copy(shot.path.clone());
        }
        if reveal {
            reveal_file(&shot.path);
        }
        if set_icon && let Some(world) = world_dir {
            self.icon_for = Some((shot.path.clone(), world));
            let _ = self.jobs.send(Job::Full(shot.path.clone(), Purpose::Icon));
        }
        if trash {
            match nailong_core::paths::to_trash(&shot.path) {
                Ok(()) => {
                    self.events.push(ViewerEvent::Trashed(shot.path.clone()));
                    self.list.retain(|s| s.path != shot.path);
                    self.thumbs.remove(&shot.path);
                    if let Some(viewer) = &mut self.viewer {
                        viewer.seq.retain(|p| p != &shot.path);
                        viewer.full = None;
                        viewer.trash_armed = false;
                        if viewer.seq.is_empty() {
                            self.viewer = None;
                        } else {
                            viewer.pos = viewer.pos.min(viewer.seq.len() - 1);
                            let next = viewer.seq[viewer.pos].clone();
                            let _ = self.jobs.send(Job::Full(next, Purpose::View));
                        }
                    }
                }
                Err(e) => self.events.push(ViewerEvent::TrashFailed(e.to_string())),
            }
            ctx.request_repaint();
        }
        let jump = jump_path.and_then(|p| self.viewer.as_ref()?.seq.iter().position(|q| q == &p));
        if let Some(target) = jump {
            self.show_pos(target);
            ctx.request_repaint();
        } else if step != 0 {
            self.show_pos((pos as i32 + step) as usize);
            ctx.request_repaint();
        }
        if close && let Some(viewer) = &mut self.viewer {
            if viewer.closing_at.is_none() {
                viewer.closing_at = Some(now);
            }
            ctx.request_repaint();
        }
    }
}

/// 一张图在哪：「生存一号 · Fabric 1.21.1」「mc.hypixel.net · 1.20.1」「Fabric 1.21.1」。
pub(crate) fn place_line(shot: &Shot) -> String {
    match (&shot.meta.world, &shot.meta.server) {
        (Some(world), _) => format!("{world} · {}", shot.version),
        (None, Some(server)) => format!("{server} · {}", shot.version),
        _ => shot.version.clone(),
    }
}

/// 设为世界图标：从中间裁成正方形，缩到 64×64，写成存档里的 `icon.png`。
fn world_icon(image: &image::RgbaImage, world: &std::path::Path) -> Option<()> {
    let side = image.width().min(image.height());
    let x = (image.width() - side) / 2;
    let y = (image.height() - side) / 2;
    let square = image::imageops::crop_imm(image, x, y, side, side).to_image();
    let icon = image::imageops::resize(&square, 64, 64, image::imageops::FilterType::Lanczos3);
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(icon)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    nailong_core::paths::write_atomic(&world.join("icon.png"), &bytes).ok()
}

/// 让缩略图铺满格子（多出来的裁掉），不留黑边。
pub(crate) fn cover_uv(texture: egui::Vec2, cell: egui::Vec2) -> egui::Rect {
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
pub(crate) fn badge(ui: &egui::Ui, rect: egui::Rect, text: &str, alpha: f32) {
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
    fn caches_thumbnails_and_sets_world_icons() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("cairn-ui-shots-{}-{n}", std::process::id()));
        let dir = base.join("screenshots");
        std::fs::create_dir_all(&dir).unwrap();
        let image = image::RgbaImage::from_pixel(640, 360, image::Rgba([90, 120, 200, 255]));
        let path = dir.join("2026-11-07_21.34.12.png");
        image.save(&path).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let cache = base.join("thumbs");
        let thumb = make_thumb(&cache, &path, size).unwrap();
        assert_eq!(thumb.size, [320, 180]);
        assert!(thumb_path(&cache, &path, size).is_file());
        assert_eq!(make_thumb(&cache, &path, size).unwrap().size, [320, 180]);
        let world = base.join("saves/生存一号");
        std::fs::create_dir_all(&world).unwrap();
        world_icon(&image, &world).unwrap();
        let icon = decode(&world.join("icon.png")).unwrap();
        assert_eq!((icon.width(), icon.height()), (64, 64));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn labels_times_like_a_photo_app() {
        let now = chrono::Local::now().naive_local();
        assert!(when_label(&now).starts_with("今天 "));
        let old =
            NaiveDateTime::parse_from_str("2020-03-01 08:05:00", "%Y-%m-%d %H:%M:%S").unwrap();
        assert_eq!(when_label(&old), "2020年3月1日 08:05");
    }
}
