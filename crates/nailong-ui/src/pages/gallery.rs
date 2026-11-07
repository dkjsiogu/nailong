//! 截图页：每张图都有时间、地点、人物——地点是哪个版本的哪个存档或服务器，人物是哪个账户。
//!
//! - **总览**：一共多少张、跨了多久、去过几个地方、有几个人；最近几张；每个月截了多少；
//!   截得最多的地方；每个人。哪个数字都能点进去。
//! - **时间**：最近一周按天，再早按月，像手机相册。Ctrl+滚轮调大小。
//! - **地点**：先按版本，再按存档或服务器，一个地方一本相簿。
//! - **人物**：每个账户截了哪些。
//! - 点进相簿、人物或某个月，就是「时间」视图加上筛选；筛选条件显示成可以删掉的标签。
//!   搜索框认存档名、服务器、人名、版本和日期。

use crate::pages::shots::{Shot, Shots, weekday};
use crate::prelude::*;
use chrono::{Datelike, NaiveDate};

/// 一张图拍在哪：哪个游戏目录里的哪个世界，或者哪个服务器。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Place {
    World {
        game_dir: PathBuf,
        name: String,
    },
    Server(String),
    /// 不知道在哪：记下它属于哪个版本（游戏目录）。
    Unknown(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum GalleryTab {
    #[default]
    Overview,
    Time,
    Places,
    People,
}

/// 「时间」视图上的筛选。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) version: Option<String>,
    pub(crate) place: Option<Place>,
    /// `Some(None)` 是「不知道是谁」。
    pub(crate) player: Option<Option<String>>,
    pub(crate) month: Option<(i32, u32)>,
}

/// 截图页的状态。底座 [`Shots`] 也给启动台那一排用。
#[derive(Default)]
pub(crate) struct GalleryState {
    pub(crate) shots: Option<Shots>,
    /// 下一帧重新列一遍（玩完一局、回到窗口、进这一页）。
    pub(crate) stale: bool,
    pub(crate) tab: GalleryTab,
    pub(crate) filter: Filter,
    pub(crate) query: String,
    /// 时间视图的缩略图大小：0 小、1 中、2 大。
    pub(crate) size: u8,
    /// 每个游戏目录 `servers.dat` 里的服务器名。
    pub(crate) server_names: HashMap<String, String>,
}

pub(crate) fn place_of(shot: &Shot) -> Place {
    match (&shot.meta.world, &shot.meta.server) {
        (Some(world), _) => Place::World {
            game_dir: shot.game_dir.clone(),
            name: world.clone(),
        },
        (None, Some(server)) => Place::Server(server.clone()),
        _ => Place::Unknown(shot.game_dir.clone()),
    }
}

impl GalleryState {
    fn place_title(&self, place: &Place) -> String {
        match place {
            Place::World { name, .. } => name.clone(),
            Place::Server(address) => self
                .server_names
                .get(address)
                .cloned()
                .unwrap_or_else(|| address.clone()),
            Place::Unknown(_) => "未知地点".into(),
        }
    }

    fn matches(&self, shot: &Shot) -> bool {
        let f = &self.filter;
        if f.version.as_ref().is_some_and(|v| v != &shot.version) {
            return false;
        }
        if f.place.as_ref().is_some_and(|p| p != &place_of(shot)) {
            return false;
        }
        if f.player.as_ref().is_some_and(|p| p != &shot.meta.player) {
            return false;
        }
        if f.month
            .is_some_and(|(y, m)| shot.time.year() != y || shot.time.month() != m)
        {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let place = self.place_title(&place_of(shot));
        let date = shot.time.date();
        let haystack = format!(
            "{} {} {} {} {} {}年{}月{}日 {}月{}日 {} {}",
            place,
            shot.meta.server.clone().unwrap_or_default(),
            shot.meta.player.clone().unwrap_or_default(),
            shot.meta.account.clone().unwrap_or_default(),
            shot.version,
            date.year(),
            date.month(),
            date.day(),
            date.month(),
            date.day(),
            date.format("%Y-%m-%d"),
            weekday(date),
        )
        .to_lowercase();
        query.split_whitespace().all(|word| haystack.contains(word))
    }
}

/// 时间视图的一段：一天或一个月。
struct Section {
    title: String,
    sub: String,
    right: String,
    items: Vec<usize>,
}

fn time_sections(gallery: &GalleryState, list: &[Shot], items: &[usize]) -> Vec<Section> {
    let today = chrono::Local::now().date_naive();
    #[derive(PartialEq)]
    enum Key {
        Day(NaiveDate),
        Month(i32, u32),
    }
    let key_of = |date: NaiveDate| {
        let days = (today - date).num_days();
        if (0..7).contains(&days) {
            Key::Day(date)
        } else {
            Key::Month(date.year(), date.month())
        }
    };
    let mut out: Vec<(Key, Vec<usize>)> = Vec::new();
    for &i in items {
        let key = key_of(list[i].time.date());
        match out.last_mut() {
            Some((last, group)) if *last == key => group.push(i),
            _ => out.push((key, vec![i])),
        }
    }
    out.into_iter()
        .map(|(key, items)| {
            let (title, sub) = match key {
                Key::Day(date) => {
                    let days = (today - date).num_days();
                    let md = format!("{}月{}日", date.month(), date.day());
                    match days {
                        0 => ("今天".into(), format!("{md} {}", weekday(date))),
                        1 => ("昨天".into(), format!("{md} {}", weekday(date))),
                        _ => (md, weekday(date).to_string()),
                    }
                }
                Key::Month(y, m) if y == today.year() => (format!("{m} 月"), String::new()),
                Key::Month(y, m) => (format!("{y} 年 {m} 月"), String::new()),
            };
            let mut places: Vec<(String, usize)> = Vec::new();
            for &i in &items {
                let name = gallery.place_title(&place_of(&list[i]));
                match places.iter_mut().find(|(n, _)| n == &name) {
                    Some((_, count)) => *count += 1,
                    None => places.push((name, 1)),
                }
            }
            places.sort_by_key(|p| std::cmp::Reverse(p.1));
            let names: Vec<String> = places.iter().take(2).map(|(n, _)| n.clone()).collect();
            let more = if places.len() > 2 { " 等" } else { "" };
            let right = format!("{}{more} · {} 张", names.join("、"), items.len());
            Section {
                title,
                sub,
                right,
                items,
            }
        })
        .collect()
}

/// 一本相簿：一个地方（或一个人）的截图，最新的在前。
struct Album {
    place: Place,
    title: String,
    version: String,
    items: Vec<usize>,
}

impl App {
    /// 每个游戏目录和它对应的版本名；几个版本共用一个目录时写「几个版本共用」。
    pub(crate) fn shot_dirs(&self) -> Vec<(PathBuf, String)> {
        let mut out: Vec<(PathBuf, Vec<String>)> = Vec::new();
        for row in &self.rows {
            if row.hidden {
                continue;
            }
            let dir = row
                .game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf());
            match out.iter_mut().find(|(d, _)| d == &dir) {
                Some((_, labels)) => labels.push(row.label.clone()),
                None => out.push((dir, vec![row.label.clone()])),
            }
        }
        out.into_iter()
            .map(|(dir, labels)| {
                let label = if labels.len() == 1 {
                    labels[0].clone()
                } else {
                    format!("{} 个版本共用", labels.len())
                };
                (dir, label)
            })
            .collect()
    }

    /// 每帧：收后台结果；该重新列的时候在后台列一遍；处理看大图时做完的事。
    pub(crate) fn tick_shots(&mut self, ctx: &egui::Context) {
        let wanted = self.gallery.shots.is_some() || self.show_gallery;
        if !wanted {
            return;
        }
        if self.gallery.shots.is_none() {
            self.gallery.shots = Some(Shots::new(ctx, self.root.clone()));
            self.gallery.stale = true;
        }
        let now = ctx.input(|i| i.time);
        let playing = matches!(self.busy, Some(Task::Launch));
        let dirs = if self.gallery.stale || playing {
            Some(self.shot_dirs())
        } else {
            None
        };
        let Some(shots) = &mut self.gallery.shots else {
            return;
        };
        shots.sync(ctx);
        // 游戏开着时几秒看一次，新截的图回到窗口就在；平时只在需要时列。
        if let Some(dirs) = dirs
            && (self.gallery.stale || shots.age(now) > 4.0)
        {
            if self.gallery.stale {
                let mut names = HashMap::new();
                for (dir, _) in &dirs {
                    names.extend(nailong_core::shots::server_names(dir));
                }
                self.gallery.server_names = names;
            }
            self.gallery.stale = false;
            shots.rescan(ctx, dirs);
        }
        if playing {
            ctx.request_repaint_after(Duration::from_secs(4));
        }
        for event in shots.take_events() {
            match event {
                shots::ViewerEvent::Trashed(path) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.notice = Some(format!("已移到回收站：{name}"));
                }
                shots::ViewerEvent::TrashFailed(e) | shots::ViewerEvent::IconFailed(e) => {
                    self.error = Some(e);
                }
                shots::ViewerEvent::IconSet(world) => {
                    self.notice = Some(format!("已设为「{world}」的世界图标"));
                    self.home.cache = None;
                }
            }
        }
    }

    /// 进截图页；`filter` 不为空时直接到「时间」视图看筛出来的。
    pub(crate) fn open_gallery(&mut self, filter: Option<Filter>) {
        if self.page_locked() {
            return;
        }
        self.show_mods = false;
        self.show_saves = false;
        self.show_manage = false;
        self.show_install = false;
        self.show_gallery = true;
        self.gallery.stale = true;
        if let Some(filter) = filter {
            self.gallery.filter = filter;
            self.gallery.tab = GalleryTab::Time;
            self.gallery.query.clear();
        }
    }

    /// 账户名 → 皮肤的脸（看大图和人物页用）。
    pub(crate) fn player_faces(
        &mut self,
        ctx: &egui::Context,
    ) -> HashMap<String, egui::TextureHandle> {
        let accounts: Vec<(String, Option<String>)> = self
            .accounts
            .list
            .iter()
            .map(|a| (a.name.clone(), a.skin.clone()))
            .collect();
        let mut out = HashMap::new();
        for (name, skin) in accounts {
            if let Some(face) = self.face(ctx, skin.as_deref()) {
                out.insert(name, face);
            }
        }
        out
    }

    pub(crate) fn gallery_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let faces = self.player_faces(&ctx);
        let Some(mut shots) = self.gallery.shots.take() else {
            ui.label(egui::RichText::new("正在读取…").color(t::dim()));
            return;
        };
        let visible: Vec<usize> = (0..shots.list.len())
            .filter(|&i| self.gallery.matches(&shots.list[i]))
            .collect();
        let total = shots.list.len();
        let versions: BTreeSet<&str> = shots.list.iter().map(|s| s.version.as_str()).collect();
        let version_count = versions.len();
        let pad = 36.0;
        let mut open_folder_dir: Option<PathBuf> = None;
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: pad as i8,
                right: pad as i8,
                top: 24,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("截图")
                            .size(24.0)
                            .family(display_family()),
                    );
                    ui.add_space(6.0);
                    if total > 0 {
                        ui.label(
                            egui::RichText::new(format!("{total} 张 · {version_count} 个版本"))
                                .size(13.0)
                                .color(t::dim()),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if action_link(ui, "打开文件夹").clicked() {
                            let dir = visible
                                .first()
                                .map(|&i| shots.list[i].game_dir.clone())
                                .unwrap_or_else(|| self.root.dir().to_path_buf())
                                .join("screenshots");
                            open_folder_dir = Some(dir);
                        }
                        ui.add_space(8.0);
                        let search = search_field(
                            ui,
                            &mut self.gallery.query,
                            "搜索存档、服务器、人或日期",
                            240.0,
                        );
                        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                            search.request_focus();
                        }
                        if !self.gallery.query.trim().is_empty()
                            && self.gallery.tab != GalleryTab::Time
                        {
                            self.gallery.tab = GalleryTab::Time;
                        }
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (GalleryTab::Overview, "总览"),
                        (GalleryTab::Time, "时间"),
                        (GalleryTab::Places, "地点"),
                        (GalleryTab::People, "人物"),
                    ] {
                        if content_tab(ui, label, 0, self.gallery.tab == tab).clicked() {
                            self.gallery.tab = tab;
                            if tab != GalleryTab::Time {
                                self.gallery.filter = Filter::default();
                                self.gallery.query.clear();
                            }
                        }
                    }
                    if self.gallery.tab == GalleryTab::Time {
                        ui.add_space(10.0);
                        self.filter_chips(ui);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new("Ctrl + 滚轮调大小")
                                    .size(12.5)
                                    .color(t::dim()),
                            );
                        });
                    }
                });
                ui.add_space(12.0);
            });
        let line_y = ui.cursor().top();
        ui.painter().hline(
            ui.max_rect().x_range(),
            line_y,
            egui::Stroke::new(1.0, t::hairline()),
        );
        ui.add_space(1.0);
        if !shots.loaded {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                busy_spinner(ui);
                ui.label(
                    egui::RichText::new("正在读取截图…")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
        } else if total == 0 {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("还没有截图").size(18.0));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("在游戏里按 F2 截图")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
        } else {
            match self.gallery.tab {
                GalleryTab::Overview => self.overview(ui, &mut shots, &faces),
                GalleryTab::Time => self.time_view(ui, &mut shots, &visible, &faces),
                GalleryTab::Places => self.places_view(ui, &mut shots),
                GalleryTab::People => self.people_view(ui, &mut shots, &faces),
            }
        }
        self.gallery.shots = Some(shots);
        if let Some(dir) = open_folder_dir {
            let _ = std::fs::create_dir_all(&dir);
            open_folder(&dir);
        }
    }

    /// 时间视图顶上的筛选标签，点 × 去掉。
    fn filter_chips(&mut self, ui: &mut egui::Ui) {
        let f = self.gallery.filter.clone();
        let mut chips: Vec<(String, u8)> = Vec::new();
        if let Some(version) = &f.version {
            chips.push((version.clone(), 0));
        }
        if let Some(place) = &f.place {
            chips.push((self.gallery.place_title(place), 1));
        }
        if let Some(player) = &f.player {
            chips.push((player.clone().unwrap_or_else(|| "未知玩家".into()), 2));
        }
        if let Some((y, m)) = f.month {
            chips.push((format!("{y} 年 {m} 月"), 3));
        }
        for (label, kind) in chips {
            if chip(ui, &format!("{label}  ×"), true)
                .on_hover_text("移除筛选")
                .clicked()
            {
                match kind {
                    0 => self.gallery.filter.version = None,
                    1 => self.gallery.filter.place = None,
                    2 => self.gallery.filter.player = None,
                    _ => self.gallery.filter.month = None,
                }
            }
        }
    }

    fn overview(
        &mut self,
        ui: &mut egui::Ui,
        shots: &mut Shots,
        faces: &HashMap<String, egui::TextureHandle>,
    ) {
        let list = shots.list.clone();
        let total = list.len();
        let pad = 36.0;
        let mut go: Option<(GalleryTab, Filter)> = None;
        let mut open: Option<(Vec<PathBuf>, PathBuf, egui::Rect)> = None;
        let albums = self.albums(&list);
        let people = people(&list);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: pad as i8,
                        right: pad as i8,
                        top: 18,
                        bottom: 24,
                    })
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        // 一行四格：多少张、跨了多久、几个地方、几个人。
                        let oldest = list.last().map(|s| s.time.date());
                        let span = match oldest {
                            Some(date) if date.year() == chrono::Local::now().year() => {
                                format!("{} 月至今", date.month())
                            }
                            Some(date) => format!("{} 年 {} 月至今", date.year(), date.month()),
                            None => String::new(),
                        };
                        let versions: BTreeSet<&str> =
                            list.iter().map(|s| s.version.as_str()).collect();
                        let cells = [
                            (format!("{total}"), "张截图".to_string(), GalleryTab::Time),
                            (span, "时间".to_string(), GalleryTab::Time),
                            (
                                format!("{} 个地点", albums.len()),
                                format!("{} 个版本", versions.len()),
                                GalleryTab::Places,
                            ),
                            (
                                format!("{} 个人", people.len()),
                                "账户".to_string(),
                                GalleryTab::People,
                            ),
                        ];
                        let cell_w = width / cells.len() as f32;
                        let (row, _) =
                            ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::hover());
                        for (i, (value, label, tab)) in cells.iter().enumerate() {
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(row.left() + i as f32 * cell_w, row.top()),
                                egui::vec2(cell_w, 64.0),
                            );
                            let response = ui.interact(
                                rect,
                                egui::Id::unique(("overview_cell", i)),
                                egui::Sense::click(),
                            );
                            list_row_fill(ui, rect.shrink(2.0), false, response.hovered());
                            if i > 0 {
                                ui.painter().vline(
                                    rect.left(),
                                    rect.y_range().shrink(10.0),
                                    egui::Stroke::new(1.0, t::hairline()),
                                );
                            }
                            ui.painter().text(
                                rect.left_top() + egui::vec2(14.0, 12.0),
                                egui::Align2::LEFT_TOP,
                                value,
                                egui::FontId::new(20.0, light_family()),
                                t::ink(),
                            );
                            ui.painter().text(
                                rect.left_top() + egui::vec2(14.0, 40.0),
                                egui::Align2::LEFT_TOP,
                                label,
                                egui::FontId::proportional(13.0),
                                t::dim(),
                            );
                            if response
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                go = Some((*tab, Filter::default()));
                            }
                        }

                        // 最近。
                        ui.add_space(18.0);
                        if section_title(ui, "最近", "全部 ›") {
                            go = Some((GalleryTab::Time, Filter::default()));
                        }
                        let columns = 6usize;
                        let gap = 6.0;
                        let tile_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
                        let tile_h = tile_w * 9.0 / 16.0;
                        let recent: Vec<&Shot> = list.iter().take(columns * 2).collect();
                        let seq: Vec<PathBuf> = list.iter().map(|s| s.path.clone()).collect();
                        let rows = recent.len().div_ceil(columns);
                        let (grid, _) = ui.allocate_exact_size(
                            egui::vec2(width, rows as f32 * (tile_h + gap)),
                            egui::Sense::hover(),
                        );
                        for (i, shot) in recent.into_iter().enumerate() {
                            let rect = egui::Rect::from_min_size(
                                grid.min
                                    + egui::vec2(
                                        (i % columns) as f32 * (tile_w + gap),
                                        (i / columns) as f32 * (tile_h + gap),
                                    ),
                                egui::vec2(tile_w, tile_h),
                            );
                            let hover = format!(
                                "{} · {}",
                                shots::when_label(&shot.time),
                                crate::pages::shots::place_line(shot)
                            );
                            if shots
                                .tile(ui, "recent", shot, rect, 3, Some(hover))
                                .clicked()
                            {
                                open = Some((seq.clone(), shot.path.clone(), rect));
                            }
                        }

                        // 每个月截了多少。
                        ui.add_space(14.0);
                        section_title(ui, "每个月", "");
                        if let Some((y, m)) = month_bars(ui, &list, width) {
                            go = Some((
                                GalleryTab::Time,
                                Filter {
                                    month: Some((y, m)),
                                    ..Default::default()
                                },
                            ));
                        }

                        // 截得最多的地方。
                        ui.add_space(18.0);
                        if section_title(ui, "地点", &format!("全部 {} 个 ›", albums.len()))
                        {
                            go = Some((GalleryTab::Places, Filter::default()));
                        }
                        let mut top = albums.iter().collect::<Vec<_>>();
                        top.sort_by_key(|a| std::cmp::Reverse(a.items.len()));
                        if let Some(place) =
                            album_grid(ui, shots, &list, &top[..top.len().min(4)], width, 4)
                        {
                            go = Some((
                                GalleryTab::Time,
                                Filter {
                                    place: Some(place),
                                    ..Default::default()
                                },
                            ));
                        }

                        // 每个人。
                        ui.add_space(18.0);
                        if section_title(ui, "人物", "全部 ›") {
                            go = Some((GalleryTab::People, Filter::default()));
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
                            for (player, items) in &people {
                                if person_chip(
                                    ui,
                                    player.as_deref(),
                                    items.len(),
                                    player.as_ref().and_then(|p| faces.get(p)),
                                ) {
                                    go = Some((
                                        GalleryTab::Time,
                                        Filter {
                                            player: Some(player.clone()),
                                            ..Default::default()
                                        },
                                    ));
                                }
                            }
                        });
                    });
            });
        if let Some((seq, path, rect)) = open {
            let now = ui.input(|i| i.time);
            shots.open_viewer(seq, &path, rect, now);
        }
        if let Some((tab, filter)) = go {
            self.gallery.tab = tab;
            self.gallery.filter = filter;
        }
    }

    /// 全部地方，按版本排，每个版本里按最近一张排。
    fn albums(&self, list: &[Shot]) -> Vec<Album> {
        let mut albums: Vec<Album> = Vec::new();
        for (i, shot) in list.iter().enumerate() {
            let place = place_of(shot);
            match albums
                .iter_mut()
                .find(|a| a.place == place && a.version == shot.version)
            {
                Some(album) => album.items.push(i),
                None => albums.push(Album {
                    title: self.gallery.place_title(&place),
                    version: shot.version.clone(),
                    place,
                    items: vec![i],
                }),
            }
        }
        albums
    }

    fn time_view(
        &mut self,
        ui: &mut egui::Ui,
        shots: &mut Shots,
        visible: &[usize],
        _faces: &HashMap<String, egui::TextureHandle>,
    ) {
        let zoom = ui.input(|i| i.zoom_delta());
        if zoom > 1.02 && self.gallery.size < 2 {
            self.gallery.size += 1;
        } else if zoom < 0.98 && self.gallery.size > 0 {
            self.gallery.size -= 1;
        }
        let list = shots.list.clone();
        if visible.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("没有符合的截图")
                        .size(15.0)
                        .color(t::sub()),
                );
            });
            return;
        }
        let sections = time_sections(&self.gallery, &list, visible);
        let seq: Vec<PathBuf> = visible.iter().map(|&i| list[i].path.clone()).collect();
        let target = [118.0, 168.0, 240.0][self.gallery.size as usize];
        let pad = 36.0;
        let gap = 4.0;
        let header_h = 44.0;
        let mut open: Option<(PathBuf, egui::Rect)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let width = ui.available_width() - pad * 2.0;
                let columns = (((width + gap) / (target + gap)).floor() as usize).max(2);
                let tile_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
                let tile_h = tile_w * 9.0 / 16.0;
                let mut heights = Vec::with_capacity(sections.len());
                let mut total = 8.0;
                for section in &sections {
                    let rows = section.items.len().div_ceil(columns);
                    let h = header_h + rows as f32 * (tile_h + gap);
                    heights.push(h);
                    total += h + 6.0;
                }
                let origin = ui.min_rect().min + egui::vec2(pad, 8.0);
                ui.allocate_space(egui::vec2(ui.available_width(), total + 24.0));
                let mut y = 0.0;
                for (section, h) in sections.iter().zip(&heights) {
                    let top = y;
                    y += h + 6.0;
                    if top > viewport.max.y || top + h < viewport.min.y - 8.0 {
                        continue;
                    }
                    let head = origin + egui::vec2(0.0, top);
                    let galley = ui.painter().layout_no_wrap(
                        section.title.clone(),
                        egui::FontId::new(15.0, bold_family()),
                        t::ink(),
                    );
                    let title_w = galley.size().x;
                    ui.painter()
                        .galley(head + egui::vec2(0.0, 14.0), galley, t::ink());
                    ui.painter().text(
                        head + egui::vec2(title_w + 10.0, 16.0),
                        egui::Align2::LEFT_TOP,
                        &section.sub,
                        egui::FontId::proportional(13.0),
                        t::dim(),
                    );
                    ui.painter().text(
                        head + egui::vec2(width, 16.0),
                        egui::Align2::RIGHT_TOP,
                        &section.right,
                        egui::FontId::proportional(13.0),
                        t::sub(),
                    );
                    for (n, &i) in section.items.iter().enumerate() {
                        let row_top = top + header_h + (n / columns) as f32 * (tile_h + gap);
                        if row_top > viewport.max.y || row_top + tile_h < viewport.min.y {
                            continue;
                        }
                        let rect = egui::Rect::from_min_size(
                            origin + egui::vec2((n % columns) as f32 * (tile_w + gap), row_top),
                            egui::vec2(tile_w, tile_h),
                        );
                        let shot = &list[i];
                        let hover = format!(
                            "{} · {}{}",
                            shots::when_label(&shot.time),
                            crate::pages::shots::place_line(shot),
                            shot.meta
                                .player
                                .as_ref()
                                .map(|p| format!(" · {p}"))
                                .unwrap_or_default()
                        );
                        if shots.tile(ui, "time", shot, rect, 3, Some(hover)).clicked() {
                            open = Some((shot.path.clone(), rect));
                        }
                    }
                }
            });
        if let Some((path, rect)) = open {
            let now = ui.input(|i| i.time);
            shots.open_viewer(seq, &path, rect, now);
        }
    }

    fn places_view(&mut self, ui: &mut egui::Ui, shots: &mut Shots) {
        let list = shots.list.clone();
        let albums = self.albums(&list);
        let mut by_version: Vec<(String, Vec<&Album>)> = Vec::new();
        for album in &albums {
            match by_version.iter_mut().find(|(v, _)| v == &album.version) {
                Some((_, group)) => group.push(album),
                None => by_version.push((album.version.clone(), vec![album])),
            }
        }
        let pad = 36.0;
        let mut go: Option<Filter> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: pad as i8,
                        right: pad as i8,
                        top: 10,
                        bottom: 24,
                    })
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        for (version, group) in &by_version {
                            let count: usize = group.iter().map(|a| a.items.len()).sum();
                            let places = group
                                .iter()
                                .filter(|a| !matches!(a.place, Place::Unknown(_)))
                                .count();
                            ui.add_space(8.0);
                            if section_title(
                                ui,
                                version,
                                &if places > 0 {
                                    format!("{places} 个地点 · {count} 张 ›")
                                } else {
                                    format!("{count} 张 ›")
                                },
                            ) {
                                go = Some(Filter {
                                    version: Some(version.clone()),
                                    ..Default::default()
                                });
                            }
                            let mut sorted = group.clone();
                            sorted.sort_by_key(|a| {
                                (
                                    matches!(a.place, Place::Unknown(_)),
                                    matches!(a.place, Place::Server(_)),
                                )
                            });
                            if let Some(place) = album_grid(ui, shots, &list, &sorted, width, 4) {
                                go = Some(Filter {
                                    version: Some(version.clone()),
                                    place: Some(place),
                                    ..Default::default()
                                });
                            }
                            ui.add_space(6.0);
                        }
                    });
            });
        if let Some(filter) = go {
            self.gallery.filter = filter;
            self.gallery.tab = GalleryTab::Time;
        }
    }

    fn people_view(
        &mut self,
        ui: &mut egui::Ui,
        shots: &mut Shots,
        faces: &HashMap<String, egui::TextureHandle>,
    ) {
        let list = shots.list.clone();
        let people = people(&list);
        let pad = 36.0;
        let mut go: Option<Filter> = None;
        let mut open: Option<(Vec<PathBuf>, PathBuf, egui::Rect)> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: pad as i8,
                        right: pad as i8,
                        top: 14,
                        bottom: 24,
                    })
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        for (player, items) in &people {
                            let name = player.clone().unwrap_or_else(|| "未知玩家".into());
                            let account = player
                                .as_ref()
                                .and_then(|p| self.accounts.list.iter().find(|a| &a.name == p))
                                .map(|a| a.kind_label())
                                .or_else(|| {
                                    items.first().and_then(|&i| list[i].meta.account.clone())
                                });
                            let (head, response) = ui
                                .allocate_exact_size(egui::vec2(width, 56.0), egui::Sense::click());
                            list_row_fill(ui, head, false, response.hovered());
                            let avatar = egui::Rect::from_min_size(
                                head.left_center() + egui::vec2(8.0, -20.0),
                                egui::vec2(40.0, 40.0),
                            );
                            draw_avatar(
                                ui.painter(),
                                avatar,
                                &name,
                                player.as_ref().and_then(|p| faces.get(p)),
                            );
                            ui.painter().text(
                                avatar.right_center() + egui::vec2(12.0, -9.0),
                                egui::Align2::LEFT_CENTER,
                                &name,
                                egui::FontId::new(15.0, bold_family()),
                                t::ink(),
                            );
                            let places: BTreeSet<Place> =
                                items.iter().map(|&i| place_of(&list[i])).collect();
                            let sub = match &account {
                                Some(kind) => {
                                    format!("{kind} · {} 张 · {} 个地点", items.len(), places.len())
                                }
                                None => format!("{} 张", items.len()),
                            };
                            ui.painter().text(
                                avatar.right_center() + egui::vec2(12.0, 11.0),
                                egui::Align2::LEFT_CENTER,
                                sub,
                                egui::FontId::proportional(13.0),
                                t::dim(),
                            );
                            ui.painter().text(
                                head.right_center() - egui::vec2(10.0, 0.0),
                                egui::Align2::RIGHT_CENTER,
                                "全部 ›",
                                egui::FontId::proportional(13.0),
                                t::sub(),
                            );
                            if response
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                go = Some(Filter {
                                    player: Some(player.clone()),
                                    ..Default::default()
                                });
                            }
                            ui.add_space(4.0);
                            let columns = 6usize;
                            let gap = 4.0;
                            let tile_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
                            let tile_h = tile_w * 9.0 / 16.0;
                            let (row, _) = ui.allocate_exact_size(
                                egui::vec2(width, tile_h),
                                egui::Sense::hover(),
                            );
                            let seq: Vec<PathBuf> =
                                items.iter().map(|&i| list[i].path.clone()).collect();
                            for (n, &i) in items.iter().take(columns).enumerate() {
                                let rect = egui::Rect::from_min_size(
                                    row.min + egui::vec2(n as f32 * (tile_w + gap), 0.0),
                                    egui::vec2(tile_w, tile_h),
                                );
                                let shot = &list[i];
                                let hover = format!(
                                    "{} · {}",
                                    shots::when_label(&shot.time),
                                    crate::pages::shots::place_line(shot)
                                );
                                if shots
                                    .tile(ui, "person", shot, rect, 3, Some(hover))
                                    .clicked()
                                {
                                    open = Some((seq.clone(), shot.path.clone(), rect));
                                }
                            }
                            ui.add_space(18.0);
                        }
                    });
            });
        if let Some((seq, path, rect)) = open {
            let now = ui.input(|i| i.time);
            shots.open_viewer(seq, &path, rect, now);
        }
        if let Some(filter) = go {
            self.gallery.filter = filter;
            self.gallery.tab = GalleryTab::Time;
        }
    }
}

/// 每个人和他的截图（最新的在前），截得多的在前，「不知道是谁」放最后。
fn people(list: &[Shot]) -> Vec<(Option<String>, Vec<usize>)> {
    let mut out: Vec<(Option<String>, Vec<usize>)> = Vec::new();
    for (i, shot) in list.iter().enumerate() {
        match out.iter_mut().find(|(p, _)| p == &shot.meta.player) {
            Some((_, items)) => items.push(i),
            None => out.push((shot.meta.player.clone(), vec![i])),
        }
    }
    out.sort_by(|a, b| {
        (a.0.is_none(), std::cmp::Reverse(a.1.len()))
            .cmp(&(b.0.is_none(), std::cmp::Reverse(b.1.len())))
    });
    out
}

/// 区块标题；右边有文字时它是个入口，点了返回 true。
fn section_title(ui: &mut egui::Ui, title: &str, link: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(15.0).family(bold_family()));
        if !link.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action_link(ui, link).clicked() {
                    clicked = true;
                }
            });
        }
    });
    ui.add_space(6.0);
    clicked
}

/// 一排相簿卡片：封面、名字、张数和最近一张的日期。点了返回那个地方。
fn album_grid(
    ui: &mut egui::Ui,
    shots: &mut Shots,
    list: &[Shot],
    albums: &[&Album],
    width: f32,
    columns: usize,
) -> Option<Place> {
    let gap = 16.0;
    let card_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
    let cover_h = card_w * 9.0 / 16.0;
    let card_h = cover_h + 62.0;
    let rows = albums.len().div_ceil(columns);
    let (grid, _) = ui.allocate_exact_size(
        egui::vec2(width, rows as f32 * (card_h + gap)),
        egui::Sense::hover(),
    );
    let mut picked = None;
    for (n, album) in albums.iter().enumerate() {
        let origin = grid.min
            + egui::vec2(
                (n % columns) as f32 * (card_w + gap),
                (n / columns) as f32 * (card_h + gap),
            );
        let cover = egui::Rect::from_min_size(origin, egui::vec2(card_w, cover_h));
        let Some(&first) = album.items.first() else {
            continue;
        };
        let shot = &list[first];
        let hover = format!("{} · {} 张", album.title, album.items.len());
        let response = shots.tile(ui, "album", shot, cover, motion::R, Some(hover));
        let text = egui::Rect::from_min_size(
            cover.left_bottom() + egui::vec2(0.0, 6.0),
            egui::vec2(card_w, 56.0),
        );
        let text_response = ui.interact(
            text,
            egui::Id::unique(("album_text", n, &album.title, &album.version)),
            egui::Sense::click(),
        );
        let painter = ui.painter();
        let title_color = if matches!(album.place, Place::Unknown(_)) {
            t::sub()
        } else {
            t::ink()
        };
        painter.text(
            text.left_top() + egui::vec2(0.0, 2.0),
            egui::Align2::LEFT_TOP,
            short_text(&album.title, 18),
            egui::FontId::new(15.0, bold_family()),
            title_color,
        );
        let kind = match &album.place {
            Place::World { .. } => "单人",
            Place::Server(address) => {
                if address == &album.title {
                    "服务器"
                } else {
                    address.as_str()
                }
            }
            Place::Unknown(_) => "未识别",
        };
        painter.text(
            text.left_top() + egui::vec2(0.0, 23.0),
            egui::Align2::LEFT_TOP,
            short_text(&format!("{} 张 · {kind}", album.items.len()), 26),
            egui::FontId::proportional(13.0),
            t::dim(),
        );
        painter.text(
            text.left_top() + egui::vec2(0.0, 40.0),
            egui::Align2::LEFT_TOP,
            format!("最近 {}", shots::when_label(&shot.time)),
            egui::FontId::proportional(12.5),
            t::dim(),
        );
        if response.clicked()
            || text_response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            picked = Some(album.place.clone());
        }
    }
    picked
}

/// 每个月截了多少：一排细柱，最多一年。点一根返回那个月。
fn month_bars(ui: &mut egui::Ui, list: &[Shot], width: f32) -> Option<(i32, u32)> {
    let today = chrono::Local::now().date_naive();
    let mut months: Vec<(i32, u32, usize)> = Vec::new();
    let (mut y, mut m) = (today.year(), today.month());
    for _ in 0..12 {
        months.push((y, m, 0));
        if m == 1 {
            y -= 1;
            m = 12;
        } else {
            m -= 1;
        }
    }
    months.reverse();
    for shot in list {
        if let Some(entry) = months
            .iter_mut()
            .find(|(y, m, _)| shot.time.year() == *y && shot.time.month() == *m)
        {
            entry.2 += 1;
        }
    }
    let max = months.iter().map(|(_, _, c)| *c).max().unwrap_or(0).max(1);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, 86.0), egui::Sense::hover());
    let slot = width / months.len() as f32;
    let mut picked = None;
    for (i, (y, m, count)) in months.iter().enumerate() {
        let column = egui::Rect::from_min_size(
            egui::pos2(area.left() + i as f32 * slot, area.top()),
            egui::vec2(slot, 86.0),
        );
        let response = ui.interact(
            column,
            egui::Id::unique(("month_bar", y, m)),
            egui::Sense::click(),
        );
        let bar_h = if *count == 0 {
            2.0
        } else {
            6.0 + 50.0 * (*count as f32 / max as f32)
        };
        let bar = egui::Rect::from_min_max(
            egui::pos2(column.center().x - slot * 0.22, column.top() + 58.0 - bar_h),
            egui::pos2(column.center().x + slot * 0.22, column.top() + 58.0),
        );
        let color = if *count == 0 {
            t::hairline()
        } else if response.hovered() {
            t::accent()
        } else {
            t::tint(t::accent(), 0.55)
        };
        ui.painter().rect_filled(bar, 2, color);
        let label = if *m == 1 {
            format!("{y}")
        } else {
            format!("{m}月")
        };
        ui.painter().text(
            egui::pos2(column.center().x, column.top() + 70.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            if response.hovered() {
                t::ink()
            } else {
                t::dim()
            },
        );
        if *count > 0 {
            let response = response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(format!("{y} 年 {m} 月 · {count} 张"));
            if response.clicked() {
                picked = Some((*y, *m));
            }
        }
    }
    picked
}

/// 人物小卡：头像、名字、张数。
fn person_chip(
    ui: &mut egui::Ui,
    player: Option<&str>,
    count: usize,
    face: Option<&egui::TextureHandle>,
) -> bool {
    let name = player.unwrap_or("未知玩家");
    let galley =
        ui.painter()
            .layout_no_wrap(name.to_string(), egui::FontId::proportional(14.0), t::ink());
    let width = 52.0 + galley.size().x + 60.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        motion::R,
        if hovered {
            t::wash()
        } else {
            egui::Color32::TRANSPARENT
        },
        egui::Stroke::new(1.0, t::hairline()),
        egui::StrokeKind::Inside,
    );
    let avatar = egui::Rect::from_min_size(
        rect.left_center() + egui::vec2(8.0, -14.0),
        egui::vec2(28.0, 28.0),
    );
    draw_avatar(ui.painter(), avatar, name, face);
    ui.painter().galley(
        avatar.right_center() + egui::vec2(10.0, -galley.size().y / 2.0),
        galley,
        t::ink(),
    );
    ui.painter().text(
        rect.right_center() - egui::vec2(12.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        format!("{count} 张"),
        egui::FontId::proportional(13.0),
        t::dim(),
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;
    use nailong_core::shots::Meta;

    fn shot(time: &str, world: Option<&str>, player: Option<&str>, version: &str) -> Shot {
        Shot {
            path: PathBuf::from(format!("/g/screenshots/{time}.png")),
            game_dir: PathBuf::from("/g"),
            time: NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M").unwrap(),
            size: 1,
            meta: Meta {
                world: world.map(str::to_string),
                player: player.map(str::to_string),
                ..Default::default()
            },
            version: version.into(),
        }
    }

    #[test]
    fn filters_and_searches() {
        let mut gallery = GalleryState::default();
        let a = shot(
            "2026-10-03 10:00",
            Some("生存一号"),
            Some("Steve"),
            "Fabric 1.21.1",
        );
        let b = shot("2026-09-01 10:00", None, None, "1.20.1");
        assert!(gallery.matches(&a) && gallery.matches(&b));
        gallery.query = "生存".into();
        assert!(gallery.matches(&a) && !gallery.matches(&b));
        gallery.query = "10月3日".into();
        assert!(gallery.matches(&a));
        gallery.query = "steve 1.21".into();
        assert!(gallery.matches(&a) && !gallery.matches(&b));
        gallery.query.clear();
        gallery.filter.player = Some(None);
        assert!(!gallery.matches(&a) && gallery.matches(&b));
        gallery.filter = Filter {
            month: Some((2026, 10)),
            ..Default::default()
        };
        assert!(gallery.matches(&a) && !gallery.matches(&b));
    }

    #[test]
    fn groups_recent_days_then_months() {
        let gallery = GalleryState::default();
        let now = chrono::Local::now().naive_local();
        let fmt = |t: NaiveDateTime| t.format("%Y-%m-%d %H:%M").to_string();
        let list = vec![
            shot(&fmt(now), Some("生存一号"), None, "v"),
            shot(
                &fmt(now - chrono::Duration::days(1)),
                Some("空岛"),
                None,
                "v",
            ),
            shot(
                &fmt(now - chrono::Duration::days(1)),
                Some("空岛"),
                None,
                "v",
            ),
            shot("2020-05-01 10:00", None, None, "v"),
        ];
        let items: Vec<usize> = (0..list.len()).collect();
        let sections = time_sections(&gallery, &list, &items);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].title, "今天");
        assert_eq!(sections[1].title, "昨天");
        assert_eq!(sections[1].right, "空岛 · 2 张");
        assert_eq!(sections[2].title, "2020 年 5 月");
        let people = people(&list);
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].0, None);
    }
}
