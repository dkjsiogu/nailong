//! 内容 › 发现：货架、分类、搜索结果和项目卡片。

use crate::prelude::*;

impl App {
    pub(crate) fn start_mods_search(&mut self) {
        self.start_mods_search_at(0);
    }

    pub(crate) fn start_mods_search_more(&mut self) {
        let offset = self.content.results.len();
        self.start_mods_search_at(offset);
    }

    pub(crate) fn start_mods_search_at(&mut self, offset: usize) {
        if self.busy.is_none() {
            cancel::reset();
        }
        let loader = self.content.loader;
        let mc = self.content.mc.clone();
        let query = self.content.query.trim().to_string();
        let kind = self.content.kind;
        self.content.loading = true;
        self.content.searching = true;
        self.content.shelf = None;
        self.content.browse.clear();
        self.content.error = None;
        self.content.notice = None;
        if offset == 0 {
            self.content.warnings.clear();
            self.content.results.clear();
            self.content.total = 0;
            self.content.offset = 0;
            self.content.selected = None;
            self.content.versions.clear();
            self.content.version = None;
            self.content.choices.clear();
            self.content.optional.clear();
        }
        let source = self.content.source_filter;
        let (tx, rx) = mpsc::channel();
        self.content.rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::search(&query, source, mc.as_deref(), loader, kind, offset, 20)
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsSearch {
                query,
                offset,
                result,
            });
        });
    }

    /// 回到浏览页（清掉搜索态）。
    pub(crate) fn show_browse(&mut self) {
        self.content.searching = false;
        self.content.shelf = None;
        self.content.selected = None;
        self.content.results.clear();
        self.content.total = 0;
        self.content.offset = 0;
        self.content.query.clear();
        self.content.error = None;
        if self.content.browse.is_empty() && !self.content.browse_loading {
            self.start_browse();
        }
    }

    pub(crate) fn reset_browse_selection(&mut self) {
        self.content.selected = None;
        self.content.delete_confirm = None;
        self.content.versions.clear();
        self.content.versions_loading = false;
        self.content.version = None;
        self.content.choices.clear();
        self.content.optional.clear();
        self.content.results.clear();
        self.content.total = 0;
        self.content.offset = 0;
    }

    /// 按当前品类拉两排内容：热门 + 最新。
    pub(crate) fn start_browse(&mut self) {
        if self.busy.is_none() {
            cancel::reset();
        }
        let kind = self.content.kind;
        let loader = self.content.loader;
        if self.content.category.is_some()
            && self.content.source_filter == Some(mods::Source::CurseForge)
            && !mods::curseforge_available()
        {
            self.content.browse.clear();
            self.content.browse_loading = false;
            self.content.error = Some("此构建没有 CurseForge key".into());
            return;
        }
        let category = self.content.category;
        let source = self.content.source_filter;
        let mc = self.content.mc.clone();
        self.content.browse_loading = true;
        self.content.browse.clear();
        self.content.error = None;
        let (tx, rx) = mpsc::channel();
        self.content.rx = Some(rx);
        std::thread::spawn(move || {
            let load = |sort| {
                mods::browse(mods::BrowseOptions {
                    kind,
                    category,
                    source,
                    mc: mc.as_deref(),
                    loader,
                    sort,
                    offset: 0,
                    limit: 8,
                })
                .map(|page| page.hits)
            };
            let popular = load(mods::Sort::Popular);
            let newest = load(mods::Sort::Newest);
            let result = match (popular, newest) {
                (Ok(popular), Ok(newest)) => Ok(vec![
                    Shelf {
                        title: "热门",
                        sort: mods::Sort::Popular,
                        projects: popular,
                    },
                    Shelf {
                        title: "最新",
                        sort: mods::Sort::Newest,
                        projects: newest,
                    },
                ]),
                (Ok(popular), Err(_)) => Ok(vec![Shelf {
                    title: "热门",
                    sort: mods::Sort::Popular,
                    projects: popular,
                }]),
                (Err(e), _) => Err(e.to_string()),
            };
            let _ = tx.send(Msg::Browse {
                kind,
                category,
                source,
                result,
            });
        });
    }

    /// 打开某一排的完整列表（热门/最新）。
    pub(crate) fn start_shelf(&mut self, sort: mods::Sort) {
        self.content.shelf = Some(sort);
        self.content.searching = true;
        self.content.selected = None;
        self.content.results.clear();
        self.content.total = 0;
        self.content.offset = 0;
        self.content.error = None;
        self.start_shelf_more(0);
    }

    pub(crate) fn start_shelf_more(&mut self, offset: usize) {
        if self.busy.is_none() {
            cancel::reset();
        }
        let Some(sort) = self.content.shelf else {
            return;
        };
        let kind = self.content.kind;
        let loader = self.content.loader;
        let category = self.content.category;
        let source = self.content.source_filter;
        let mc = self.content.mc.clone();
        self.content.loading = true;
        let (tx, rx) = mpsc::channel();
        self.content.rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::browse(mods::BrowseOptions {
                kind,
                category,
                source,
                mc: mc.as_deref(),
                loader,
                sort,
                offset,
                limit: 24,
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(Msg::BrowseMore {
                sort,
                offset,
                result,
            });
        });
    }

    /// 拉 CurseForge 的真实品类树（每个内容类型只拉一次）。
    pub(crate) fn fetch_cf_categories(&mut self, kind: mods::ContentKind) {
        if self.content.cf_categories.contains_key(&kind) || self.content.cf_loading.contains(&kind)
        {
            return;
        }
        if !mods::curseforge_available() {
            return;
        }
        self.content.cf_loading.insert(kind);
        let (tx, rx) = mpsc::channel();
        self.content.cf_rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::curseforge_categories(kind).map_err(|e| e.to_string());
            let _ = tx.send((kind, result));
        });
    }

    /// 胶囊列表：CurseForge 品类优先，缺的再补 Modrinth 自己的品类。
    pub(crate) fn browse_chips(&self) -> Vec<(String, Option<mods::CategoryFilter>)> {
        let kind = self.content.kind;
        let mut out: Vec<(String, Option<mods::CategoryFilter>)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        if let Some(items) = self.content.cf_categories.get(&kind)
            && !items.is_empty()
        {
            let top: Vec<&mods::CurseCategory> =
                items.iter().filter(|item| item.parent.is_none()).collect();
            let picked: Vec<&mods::CurseCategory> = if top.len() >= 4 {
                top
            } else {
                items.iter().collect()
            };
            for category in picked {
                let alias = mods::category_alias(&category.slug, &category.name);
                let label = alias
                    .map(|a| a.0.to_string())
                    .unwrap_or_else(|| category.name.clone());
                if !seen.insert(label.clone()) {
                    continue;
                }
                out.push((
                    label,
                    Some(mods::CategoryFilter {
                        modrinth: alias.and_then(|a| a.1),
                        curseforge: Some(category.id),
                    }),
                ));
            }
            for category in mods::categories(kind) {
                let covered = out
                    .iter()
                    .any(|(_, filter)| filter.and_then(|f| f.modrinth) == Some(category.slug));
                if covered || !seen.insert(category.label.to_string()) {
                    continue;
                }
                out.push((
                    category.label.to_string(),
                    Some(mods::CategoryFilter {
                        modrinth: Some(category.slug),
                        curseforge: None,
                    }),
                ));
            }
        } else {
            for category in mods::categories(kind) {
                out.push((
                    category.label.to_string(),
                    Some(mods::CategoryFilter {
                        modrinth: Some(category.slug),
                        curseforge: None,
                    }),
                ));
            }
        }
        out
    }

    /// 给还没缓存的卡片排图标下载任务。
    pub(crate) fn request_icons(&mut self, projects: impl IntoIterator<Item = mods::Project>) {
        for project in projects {
            let Some(url) = project.icon_url.clone() else {
                continue;
            };
            let key = project_key(&project);
            if self.content.icons.contains_key(&key) || self.content.icons_pending.contains(&key) {
                continue;
            }
            self.content.icons_pending.insert(key.clone());
            let _ = self.content.icon_jobs.send((key, url));
        }
    }

    /// 把后台取回的图标解码成纹理（必须在 UI 线程做）。
    pub(crate) fn drain_icons(&mut self, ctx: &egui::Context) {
        let mut got = Vec::new();
        while let Ok(item) = self.content.icon_rx.try_recv() {
            got.push(item);
        }
        for (key, bytes) in got {
            self.content.icons_pending.remove(&key);
            let Ok(image) = image::load_from_memory(&bytes) else {
                continue;
            };
            let rgba = image.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            let texture = ctx.load_texture(&key, color, egui::TextureOptions::LINEAR);
            self.content.icons.insert(key, texture);
        }
    }

    pub(crate) fn browse_view(&mut self, ui: &mut egui::Ui) {
        if mods::curseforge_available() {
            let mut wanted_source: Option<Option<mods::Source>> = None;
            ui.horizontal_wrapped(|ui| {
                for (label, value) in [
                    ("全部来源", None),
                    ("Modrinth", Some(mods::Source::Modrinth)),
                    ("CurseForge", Some(mods::Source::CurseForge)),
                ] {
                    if chip(ui, label, self.content.source_filter == value).clicked() {
                        wanted_source = Some(value);
                    }
                }
            });
            if let Some(source) = wanted_source
                && source != self.content.source_filter
            {
                self.content.source_filter = source;
                self.start_browse();
            }
            ui.add_space(6.0);
        }

        let chips = self.browse_chips();
        const COLLAPSED_CHIPS: usize = 8;
        let shown = if self.content.chips_expanded {
            chips.len()
        } else {
            chips.len().min(COLLAPSED_CHIPS)
        };
        let mut wanted: Option<Option<mods::CategoryFilter>> = None;
        let mut toggle_chips = false;
        ui.horizontal_wrapped(|ui| {
            if chip(ui, "全部", self.content.category.is_none()).clicked() {
                wanted = Some(None);
            }
            for (label, filter) in chips.iter().take(shown) {
                if chip(ui, label, self.content.category == *filter).clicked() {
                    wanted = Some(*filter);
                }
            }
            if chips.len() > COLLAPSED_CHIPS {
                let label = if self.content.chips_expanded {
                    "收起".to_string()
                } else {
                    format!("更多品类 +{}", chips.len() - COLLAPSED_CHIPS)
                };
                if chip(ui, &label, false).clicked() {
                    toggle_chips = true;
                }
            }
        });
        if toggle_chips {
            self.content.chips_expanded = !self.content.chips_expanded;
        }
        if let Some(category) = wanted
            && category != self.content.category
        {
            self.content.category = category;
            self.start_browse();
        }

        ui.add_space(8.0);
        if let Some(error) = self.content.error.clone() {
            let mut reload = false;
            let empty = self.content.browse.is_empty() && !self.content.browse_loading;
            if empty {
                ui.add_space(34.0);
                ui.vertical_centered(|ui| {
                    egui::Frame::new()
                        .fill(t::surface())
                        .stroke(egui::Stroke::new(1.0, t::hairline()))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(24, 22))
                        .show(ui, |ui| {
                            ui.set_width(420.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("内容没有加载出来")
                                        .size(15.0)
                                        .family(bold_family()),
                                );
                                ui.add_space(5.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&error).size(13.0).color(t::sub()),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(12.0);
                                if motion::primary_sized(ui, "重新加载", true, 108.0, 32.0, 11.0)
                                {
                                    reload = true;
                                }
                            });
                        });
                });
            } else {
                egui::Frame::new()
                    .fill(t::tint(t::warn(), 0.11))
                    .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.41)))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(12, 9))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&error).size(13.0).color(t::accent()),
                                )
                                .wrap(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if action_link(ui, "重新加载").clicked() {
                                        reload = true;
                                    }
                                },
                            );
                        });
                    });
            }
            if reload {
                if self.content.searching {
                    self.start_mods_search();
                } else {
                    self.start_browse();
                }
            }
            if empty {
                return;
            }
        }
        if self.content.browse_loading && self.content.browse.is_empty() {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .inner_margin(egui::Margin::symmetric(20, 18))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        busy_spinner(ui);
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new("正在加载热门与最新内容…")
                                .size(13.0)
                                .family(bold_family()),
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("正在连接 Modrinth / CurseForge，第一次会慢一点")
                            .size(13.0)
                            .color(t::sub()),
                    );
                    ui.add_space(16.0);
                    for _ in 0..3 {
                        skeleton_line(ui, 12.0);
                        ui.add_space(8.0);
                    }
                });
            return;
        }

        let per_row = if ui.available_width() >= 800.0 {
            4
        } else if ui.available_width() >= 600.0 {
            3
        } else {
            2
        };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(185.0, 300.0);
        let shelves = std::mem::take(&mut self.content.browse);
        if shelves.is_empty() && self.content.error.is_none() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("这里还没有内容")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
        }
        let mut installs = Vec::new();
        let mut more: Option<mods::Sort> = None;
        let mut order = 0usize;
        for shelf in &shelves {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(shelf.title)
                        .size(15.0)
                        .family(bold_family()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("更多 ›").size(13.0).color(t::sub()),
                            )
                            .fill(t::surface())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .min_size(egui::vec2(64.0, 24.0)),
                        )
                        .clicked()
                    {
                        more = Some(shelf.sort);
                    }
                });
            });
            ui.add_space(8.0);
            for row in shelf.projects.chunks(per_row) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for project in row {
                        if let Some(chosen) = self.project_card(ui, project, card_width, order) {
                            installs.push(chosen);
                        }
                        order += 1;
                    }
                });
                ui.add_space(10.0);
            }
        }
        self.content.browse = shelves;
        if let Some(sort) = more {
            self.start_shelf(sort);
        }
        for project in installs {
            self.review_quick_install(project);
        }
        if let Some(note) = &self.content.notice {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
        }
    }

    pub(crate) fn results_view(&mut self, ui: &mut egui::Ui) {
        let mut back = false;
        ui.horizontal(|ui| {
            if ghost_button(ui, "‹ 返回").clicked() {
                back = true;
            }
            ui.add_space(10.0);
            match self.content.shelf {
                Some(sort) => {
                    let title = match sort {
                        mods::Sort::Popular => "热门",
                        mods::Sort::Newest => "最新",
                        _ => "筛选结果",
                    };
                    ui.label(
                        egui::RichText::new(title)
                            .size(24.0)
                            .family(display_family())
                            .color(t::ink()),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} 个", self.content.total))
                            .size(13.0)
                            .color(t::sub()),
                    );
                }
                None => {
                    ui.label(egui::RichText::new("搜索").size(13.0).color(t::sub()));
                    ui.label(
                        egui::RichText::new(format!(
                            "“{}”",
                            short_text(self.content.query.trim(), 26)
                        ))
                        .size(15.0)
                        .family(bold_family()),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} 个结果", self.content.total))
                            .size(13.0)
                            .color(t::sub()),
                    );
                }
            }
        });
        if back {
            self.show_browse();
            return;
        }
        ui.add_space(4.0);
        for warning in &self.content.warnings {
            ui.label(egui::RichText::new(warning).size(13.0).color(t::warn()));
        }
        if self.content.loading {
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                busy_spinner(ui);
                ui.label(egui::RichText::new("正在加载…").size(13.0).color(t::sub()));
            });
        }
        let per_row = if ui.available_width() >= 800.0 {
            4
        } else if ui.available_width() >= 600.0 {
            3
        } else {
            2
        };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(185.0, 300.0);
        let results = std::mem::take(&mut self.content.results);
        let mut installs = Vec::new();
        let mut order = 0usize;
        for chunk in results.chunks(per_row) {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for project in chunk {
                    if let Some(chosen) = self.project_card(ui, project, card_width, order) {
                        installs.push(chosen);
                    }
                    order += 1;
                }
            });
        }
        self.content.results = results;
        for project in installs {
            self.review_quick_install(project);
        }
        if !self.content.loading && self.content.results.is_empty() && self.content.error.is_none()
        {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .inner_margin(egui::Margin::symmetric(24, 28))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("没有找到匹配的内容")
                                .size(15.0)
                                .family(bold_family()),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(
                                "换个关键词，或者在「只浏览」模式下放宽游戏版本与加载器。",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        );
                        ui.add_space(12.0);
                        if primary_wide(ui, "回到浏览", true, 150.0) {
                            self.show_browse();
                        }
                    });
                });
        } else if !self.content.loading && (self.content.results.len() as u64) < self.content.total
        {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("加载更多").size(13.0).color(t::sub()),
                        )
                        .fill(t::surface())
                        .stroke(egui::Stroke::new(1.0, t::hairline()))
                        .corner_radius(motion::R)
                        .min_size(egui::vec2(120.0, 30.0)),
                    )
                    .clicked()
                {
                    if self.content.shelf.is_some() {
                        self.start_shelf_more(self.content.results.len());
                    } else {
                        self.start_mods_search_more();
                    }
                }
            });
        }
    }

    /// 画一张项目卡片；点 + 返回要安装的项目，点别处进详情。
    pub(crate) fn project_card(
        &mut self,
        ui: &mut egui::Ui,
        project: &mods::Project,
        width: f32,
        order: usize,
    ) -> Option<mods::Project> {
        #[cfg(not(debug_assertions))]
        let _ = order;
        let key = project_key(project);
        let summary_chars = ((width * 0.2) as usize).clamp(40, 62);
        let installed = self.content.installed_keys.contains(&key);
        let quick = self.content.quick.as_deref() == Some(key.as_str());
        let busy_reason = self.busy_reason();
        let busy = self.busy.is_some();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 146.0), egui::Sense::click());
        #[cfg(debug_assertions)]
        let forced_hover = std::env::var("NAILONG_DEBUG_HOVER")
            .ok()
            .is_some_and(|needle| project.name.to_lowercase().contains(&needle.to_lowercase()));
        #[cfg(not(debug_assertions))]
        let forced_hover = false;
        #[cfg(debug_assertions)]
        let cycle_hover = std::env::var("NAILONG_DEBUG_HOVER_CYCLE")
            .ok()
            .and_then(|count| count.parse::<usize>().ok())
            .is_some_and(|count| {
                let phase = ((ui.input(|input| input.time) / 1.35) as usize) % count.max(1);
                order < count && order == phase
            });
        #[cfg(not(debug_assertions))]
        let cycle_hover = false;
        let hovered = if std::env::var_os("NAILONG_DEBUG_HOVER").is_some()
            || std::env::var_os("NAILONG_DEBUG_HOVER_CYCLE").is_some()
        {
            forced_hover || cycle_hover
        } else {
            response.hovered()
        };
        let hover_t = if motion::reduced() {
            if hovered { 1.0 } else { 0.0 }
        } else {
            ui.ctx()
                .animate_bool_with_time(response.id.with("card_hover"), hovered, 0.16)
        };
        let paint = rect;
        ui.painter().rect(
            paint,
            motion::R,
            motion::mix(t::surface(), t::wash(), hover_t),
            egui::Stroke::new(1.0, motion::mix(t::hairline(), t::control(), hover_t)),
            egui::StrokeKind::Inside,
        );

        let inner = paint.shrink2(egui::vec2(12.0, 10.0));
        let mut install = None;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        child.spacing_mut().item_spacing.y = 5.0;

        child.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 9.0;
            let icon_size = egui::vec2(38.0, 38.0);
            if let Some(texture) = self.content.icons.get(&key) {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::new(
                        texture.id(),
                        icon_size,
                    ))
                    .corner_radius(motion::R),
                );
            } else {
                let (icon_rect, _) = ui.allocate_exact_size(icon_size, egui::Sense::hover());
                ui.painter().rect_filled(icon_rect, motion::R, t::wash());
                let letter = project
                    .name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_default();
                ui.painter().text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    letter,
                    egui::FontId::proportional(16.0),
                    t::sub(),
                );
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short_text(&project.name, 34))
                        .size(15.0)
                        .family(bold_family()),
                )
                .truncate(),
            );
        });
        child.add(
            egui::Label::new(
                egui::RichText::new(short_text(&project.summary, summary_chars))
                    .size(13.0)
                    .color(t::sub()),
            )
            .wrap(),
        );
        child.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} 下载", compact(project.downloads)))
                    .size(13.0)
                    .color(t::sub()),
            );
            // 窄卡片放不下来源，避免压到右边的安装按钮。
            if width >= 240.0 {
                ui.label(egui::RichText::new("·").size(13.0).color(t::sub()));
                ui.label(
                    egui::RichText::new(project.source.label())
                        .size(13.0)
                        .color(t::sub()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    ui.label(egui::RichText::new("已装").size(13.0).color(t::ok()));
                } else if quick {
                    busy_spinner(ui);
                } else if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(
                            egui::RichText::new("+")
                                .size(15.0)
                                .family(bold_family())
                                .color(t::accent()),
                        )
                        .fill(t::tint(t::accent(), 0.16))
                        .stroke(egui::Stroke::new(1.0, t::tint(t::accent(), 0.78)))
                        .corner_radius(motion::R)
                        .min_size(egui::vec2(22.0, 22.0)),
                    )
                    .on_hover_text("安装到当前版本")
                    .on_disabled_hover_text(busy_reason)
                    .clicked()
                {
                    install = Some(project.clone());
                }
            });
        });

        if response.clicked() && install.is_none() {
            self.select_mods_project(project.clone());
        }
        install
    }
}
