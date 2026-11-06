//! 内容 › 已安装：缓存秒开的列表、状态筛选、启停、删除和更新。

use crate::prelude::*;

impl App {
    pub(crate) fn start_mods_scan(&mut self) {
        self.home.cache = None;
        self.home.verified = None;
        self.play.tuning_cache.clear();
        self.version_cache = None;
        let Some(game_dir) = self.content.game_dir.clone() else {
            return;
        };
        // 先把上次的索引摆出来（读一个 JSON，几毫秒），后台再核对磁盘；
        // 核对只重读大小或修改时间变了的文件，结果回来就替换，所以不会一直显示旧的。
        if self.content.installed_dir.as_ref() != Some(&game_dir) {
            self.content.installed = installed::load(&game_dir).entries;
            self.content.installed_dir = Some(game_dir.clone());
        }
        self.content.installed_loading = self.content.installed.is_empty();
        self.content.error = None;
        self.content.dir_sig = content_signature(&game_dir);
        let (tx, rx) = mpsc::channel();
        self.content.scan_rx = Some(rx);
        std::thread::spawn(move || {
            let result = installed::scan_all(&game_dir)
                .map(|index| index.entries)
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsInstalled(result));
        });
    }

    pub(crate) fn start_mods_update_check(&mut self) {
        let Some(row) = self.content.row.clone() else {
            return;
        };
        let kind = self.content.kind;
        let loader = mod_loader(row.loader).unwrap_or(mods::Loader::Fabric);
        let entries: Vec<_> = self
            .content
            .installed
            .iter()
            .filter(|entry| {
                entry.kind == kind && entry.source.is_some() && entry.project_id.is_some()
            })
            .cloned()
            .collect();
        if entries.is_empty() {
            self.content.notice = Some("没有可检查更新的在线来源内容".into());
            return;
        }
        self.content.updates_loading = true;
        self.content.error = None;
        let (tx, rx) = mpsc::channel();
        self.content.scan_rx = Some(rx);
        let mc = row.mc.clone();
        std::thread::spawn(move || {
            let mut updates = Vec::new();
            for entry in entries {
                if let Ok(Some(candidate)) = installed::check_update(&entry, &mc, loader) {
                    updates.push(candidate);
                }
            }
            let _ = tx.send(Msg::ModsUpdates(Ok(updates)));
        });
    }

    pub(crate) fn start_mods_update(&mut self, candidates: Vec<installed::UpdateCandidate>) {
        if self.busy.is_some() || candidates.is_empty() {
            return;
        }
        let Some(game_dir) = self.content.game_dir.clone() else {
            return;
        };
        let Some(row) = self.content.row.clone() else {
            return;
        };
        let kind = self.content.kind;
        let loader = mod_loader(row.loader).unwrap_or(mods::Loader::Fabric);
        let mc = row.mc.clone();
        let dir = game_dir.join(kind.folder());
        let loader_files = loader_state_files(&self.root, &row, &game_dir);
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Mods);
        self.activity = Some(Activity::new(
            format!("{} 个更新", candidates.len()),
            "更新内容",
        ));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let tx = Arc::new(Mutex::new(tx));
        let sink: Sink = {
            let tx = tx.clone();
            from_fn(move |p| {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(Msg::Progress(p));
                }
            })
        };
        std::thread::spawn(move || {
            if let Err(e) = rollback::backup_extra(&game_dir, "更新内容前", &loader_files) {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(Msg::ModsFailed(format!("无法创建恢复点: {e}")));
                }
                return;
            }
            let old_entries = installed::load(&game_dir).entries;
            let empty = BTreeSet::new();
            let mut updated = 0usize;
            let mut error = None;
            for candidate in candidates {
                if cancel::is_cancelled() {
                    error = Some(Error::Cancelled);
                    break;
                }
                let plan = match mods::plan(&candidate.latest, &mc, loader, &empty) {
                    Ok(plan) => plan,
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                };
                if mods::download_plan(&dir, &plan, true, &sink).is_err() {
                    error = Some(Error::Install("下载更新失败".into()));
                    break;
                }
                if installed::record_versions(&game_dir, kind, &plan).is_err() {
                    error = Some(Error::Install("写入内容索引失败".into()));
                    break;
                }
                // 更新一个被禁用的项目时保持禁用，不因为换了新文件就悄悄启用。
                if !candidate.entry.enabled {
                    let index = installed::load(&game_dir);
                    for version in &plan {
                        if version.source == candidate.latest.source
                            && version.project_id == candidate.latest.project_id
                            && let Some(entry) = index.entries.iter().find(|entry| {
                                entry.kind == kind
                                    && entry.version_id.as_deref() == Some(version.id.as_str())
                            })
                        {
                            let mut copy = entry.clone();
                            let _ = installed::set_enabled(&game_dir, &mut copy, false);
                        }
                    }
                }
                for version in &plan {
                    for entry in old_entries.iter().filter(|entry| {
                        entry.source == Some(version.source)
                            && entry.project_id.as_deref() == Some(version.project_id.as_str())
                            && entry.file != version.file_name
                    }) {
                        let _ = installed::remove(&game_dir, entry);
                    }
                }
                updated += 1;
            }
            let msg = match error {
                Some(Error::Cancelled) => Msg::ModsCancelled,
                Some(e) => Msg::ModsFailed(e.to_string()),
                None => Msg::ModsDone(format!("已更新 {updated} 个项目")),
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(msg);
            }
        });
    }

    pub(crate) fn installed_view(&mut self, ui: &mut egui::Ui) {
        let Some(game_dir) = self.content.game_dir.clone() else {
            ui.add_space(36.0);
            egui::Frame::new()
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .inner_margin(egui::Margin::symmetric(24, 28))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("先选一个安装目标").size(15.0).family(bold_family()),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(
                                "内容需要一个目标版本。在上方「安装到」里选一个，或去「版本」页创建隔离版本。",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        );
                        ui.add_space(14.0);
                        if primary_wide(ui, "去版本页", true, 168.0) {
                            self.nav_go(Nav::Versions);
                        }
                    });
                });
            return;
        };

        let kind = self.content.kind;
        let mut import = false;
        let mut rescan = false;
        let mut check_updates = false;
        let mut go_browse = false;
        let mut update_all: Option<Vec<installed::UpdateCandidate>> = None;
        let mut update_one: Option<installed::UpdateCandidate> = None;
        let mut toggles: Vec<(installed::InstalledEntry, bool)> = Vec::new();
        let mut deletes: Vec<installed::InstalledEntry> = Vec::new();
        let mut open_dir = false;
        let mut reveal: Option<PathBuf> = None;
        let mut open_rollback = false;

        let entries_all: Vec<installed::InstalledEntry> = self
            .content
            .installed
            .iter()
            .filter(|entry| entry.kind == kind)
            .cloned()
            .collect();
        let enabled = entries_all.iter().filter(|entry| entry.enabled).count();
        let disabled = entries_all.len().saturating_sub(enabled);
        let updates: Vec<installed::UpdateCandidate> = self
            .content
            .updates
            .iter()
            .filter(|candidate| candidate.entry.kind == kind)
            .cloned()
            .collect();
        let groups = installed::duplicates(&entries_all);
        // 重复的按组排在一起，同一个 Mod 的两份挨着，方便留一份。
        let duplicate_rank: HashMap<String, usize> = groups
            .iter()
            .enumerate()
            .flat_map(|(rank, (_, items))| {
                items.iter().map(move |entry| (entry.file.clone(), rank))
            })
            .collect();
        let status = self.content.status;
        let query = self.content.filter.trim().to_lowercase();
        let mut entries: Vec<installed::InstalledEntry> = entries_all
            .iter()
            .filter(|entry| match status {
                StatusFilter::All => true,
                StatusFilter::Enabled => entry.enabled,
                StatusFilter::Disabled => !entry.enabled,
                StatusFilter::Updates => updates.iter().any(|c| c.entry.file == entry.file),
                StatusFilter::Duplicates => duplicate_rank.contains_key(&entry.file),
            })
            .filter(|entry| {
                query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.file.to_lowercase().contains(&query)
                    || entry
                        .mod_id
                        .as_deref()
                        .is_some_and(|id| id.to_lowercase().contains(&query))
            })
            .cloned()
            .collect();
        if status == StatusFilter::Duplicates {
            entries.sort_by_key(|entry| duplicate_rank.get(&entry.file).copied());
        }
        let filtered = status != StatusFilter::All || !query.is_empty();

        // 第一行：搜索在左，文件相关的动作在右。
        let mut wanted_status: Option<StatusFilter> = None;
        ui.horizontal(|ui| {
            let search = search_field(
                ui,
                &mut self.content.filter,
                &if kind.label().is_ascii() {
                    format!("搜索 {} 名称或文件名", kind.label())
                } else {
                    format!("搜索{}名称或文件名", kind.label())
                },
                240.0,
            );
            if std::mem::take(&mut self.content.focus_search) {
                search.request_focus();
            }
            if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.content.filter.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if accent_outline(ui, "＋ 导入文件")
                    .on_hover_text("也可以直接把文件拖进窗口，一次可以拖很多个")
                    .clicked()
                {
                    import = true;
                }
                ui.add_space(2.0);
                if ui
                    .add_enabled_ui(!entries_all.is_empty(), |ui| quick_action(ui, "检查更新"))
                    .inner
                    .clicked()
                {
                    check_updates = true;
                }
                if quick_action(ui, "打开文件夹")
                    .on_hover_text(game_dir.join(kind.folder()).display().to_string())
                    .clicked()
                {
                    open_dir = true;
                }
            });
        });

        // 第二行：状态就是筛选。只显示有东西的状态。
        if !entries_all.is_empty() {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                for (filter, count, tone) in [
                    (StatusFilter::All, entries_all.len(), None),
                    (StatusFilter::Enabled, enabled, None),
                    (StatusFilter::Disabled, disabled, Some(t::warn())),
                    (StatusFilter::Updates, updates.len(), Some(t::accent())),
                    (
                        StatusFilter::Duplicates,
                        duplicate_rank.len(),
                        Some(t::warn()),
                    ),
                ] {
                    let show = match filter {
                        StatusFilter::All => true,
                        StatusFilter::Enabled | StatusFilter::Disabled => disabled > 0,
                        _ => count > 0,
                    };
                    if !show && status != filter {
                        continue;
                    }
                    if status_chip(ui, filter.label(), count, status == filter, tone).clicked() {
                        wanted_status = Some(if status == filter {
                            StatusFilter::All
                        } else {
                            filter
                        });
                    }
                }
                if filtered {
                    inline_dot(ui);
                    ui.label(
                        egui::RichText::new(format!("显示 {} 项", entries.len()))
                            .size(13.0)
                            .color(t::sub()),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if action_link(ui, "重新扫描")
                        .on_hover_text("回到窗口时会自动发现文件夹里的变化，一般不用点")
                        .clicked()
                    {
                        rescan = true;
                    }
                    if action_link(ui, "恢复点")
                        .on_hover_text("恢复 Mod / 配置到之前的状态")
                        .clicked()
                    {
                        open_rollback = true;
                    }
                });
            });
        }
        if let Some(filter) = wanted_status {
            self.content.status = filter;
            self.content.delete_confirm = None;
        }

        if self.content.updates_loading {
            ui.add_space(9.0);
            ui.horizontal(|ui| {
                busy_spinner(ui);
                ui.label(
                    egui::RichText::new("检查版本更新…")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
        }
        if let Some(note) = &self.content.notice {
            ui.add_space(9.0);
            ui.add(egui::Label::new(egui::RichText::new(note).size(13.0).color(t::ok())).wrap());
        }
        if let Some(err) = &self.content.error {
            ui.add_space(9.0);
            ui.add(egui::Label::new(egui::RichText::new(err).size(13.0).color(t::danger())).wrap());
        }

        let loaded = !self.content.installed_loading;
        if !loaded {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .inner_margin(egui::Margin::symmetric(16, 18))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        busy_spinner(ui);
                        ui.label(
                            egui::RichText::new("正在扫描内容目录…")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
        } else if entries_all.is_empty() {
            ui.add_space(18.0);
            egui::Frame::new()
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .inner_margin(egui::Margin::symmetric(24, 30))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(format!("还没有安装 {}", kind.label()))
                                .size(15.0)
                                .family(bold_family()),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "去「发现」挑一个，或者把 {} 文件拖进窗口。",
                                content_ext(kind)
                            ))
                            .size(13.0)
                            .color(t::sub()),
                        );
                        ui.add_space(14.0);
                        if primary_wide(ui, &format!("去发现 {}", kind.label()), true, 168.0) {
                            go_browse = true;
                        }
                        ui.add_space(3.0);
                        if action_link(ui, "或导入本地文件").clicked() {
                            import = true;
                        }
                    });
                });
        } else {
            if !updates.is_empty() {
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(t::tint(t::warn(), 0.13))
                    .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.49)))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("↑").size(15.0).color(t::accent()));
                            ui.add_space(2.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{} 项有新版本", updates.len()))
                                        .size(13.0)
                                        .family(bold_family()),
                                );
                                ui.label(
                                    egui::RichText::new("逐个更新，旧文件会一并清理")
                                        .size(13.0)
                                        .color(t::sub()),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if accent_outline(ui, "全部更新").clicked() {
                                        update_all = Some(updates.clone());
                                    }
                                },
                            );
                        });
                    });
            }

            if !groups.is_empty() && status != StatusFilter::Duplicates {
                let names = groups
                    .iter()
                    .map(|(_, items)| items[0].name.clone())
                    .collect::<Vec<_>>()
                    .join("、");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    status_dot(ui, false);
                    ui.add_space(2.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!(
                                "{} 个 Mod 装了不止一份：{}",
                                groups.len(),
                                short_text(&names, 60)
                            ))
                            .size(13.0)
                            .color(t::warn()),
                        )
                        .truncate(),
                    );
                    if action_link(ui, "只看这些").clicked() {
                        self.content.status = StatusFilter::Duplicates;
                        self.content.delete_confirm = None;
                    }
                });
            } else if status == StatusFilter::Duplicates && !groups.is_empty() {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new(
                        "同一个 Mod 的几份挨在一起：留一份，其余禁用或删除，游戏才不会报重复",
                    )
                    .size(13.0)
                    .color(t::sub()),
                );
            }

            if entries.is_empty() {
                ui.add_space(18.0);
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(24, 26))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            let title = match (query.is_empty(), status) {
                                (false, StatusFilter::All) => {
                                    format!("没有匹配「{}」的内容", self.content.filter.trim())
                                }
                                (false, filter) => format!(
                                    "{}里没有匹配「{}」的",
                                    filter.label(),
                                    self.content.filter.trim()
                                ),
                                (true, filter) => {
                                    format!("没有{}的{}", filter.label(), kind.label())
                                }
                            };
                            ui.label(egui::RichText::new(title).size(15.0).family(bold_family()));
                            ui.add_space(8.0);
                            if action_link(ui, "清除筛选，看全部").clicked() {
                                self.content.filter.clear();
                                self.content.status = StatusFilter::All;
                                self.content.delete_confirm = None;
                            }
                        });
                    });
            } else {
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        for (index, entry) in entries.iter().enumerate() {
                            let size = entry.size.or_else(|| content_size(&game_dir, kind, entry));
                            let candidate = updates.iter().find(|c| c.entry.file == entry.file);
                            let confirming =
                                self.content.delete_confirm.as_deref() == Some(entry.file.as_str());
                            let action = installed_row(
                                ui,
                                entry,
                                size,
                                candidate.map(|c| c.latest.version_number.as_str()),
                                confirming,
                                self.content.recent.contains(&entry.file),
                                index + 1 == entries.len(),
                            );
                            if let Some(enabled) = action.toggle {
                                toggles.push((entry.clone(), enabled));
                            }
                            if action.reveal {
                                reveal = Some(installed::path_of(&game_dir, entry));
                            }
                            if action.ask_delete {
                                self.content.delete_confirm = Some(entry.file.clone());
                            }
                            if action.cancel_delete {
                                self.content.delete_confirm = None;
                            }
                            if action.delete {
                                deletes.push(entry.clone());
                            }
                            if action.update
                                && let Some(candidate) = candidate
                            {
                                update_one = Some(candidate.clone());
                            }
                        }
                    });
            }
        }

        let loader_files = self
            .content
            .row
            .as_ref()
            .map(|row| loader_state_files(&self.root, row, &game_dir))
            .unwrap_or_default();
        let wants_write = import
            || !toggles.is_empty()
            || !deletes.is_empty()
            || update_all.is_some()
            || update_one.is_some()
            || open_rollback;
        if wants_write
            && let Some(reason) =
                self.write_block(self.content.row.as_ref().map(|row| row.id.as_str()))
        {
            self.content.notice = None;
            self.content.error = Some(reason.into());
            import = false;
            toggles.clear();
            deletes.clear();
            update_all = None;
            update_one = None;
            open_rollback = false;
        }
        if import {
            let files = pick_content_files(kind);
            if !files.is_empty() {
                // 从对话框选的，类别就是当前页：资源包也可能是 .jar。
                let plan = DropPlan {
                    target: self.drop_target(),
                    items: files.into_iter().map(|file| (file, kind)).collect(),
                    ..Default::default()
                };
                self.import_dropped(plan);
            }
        }
        if rescan {
            self.content.notice = None;
            self.start_mods_scan();
        }
        if check_updates {
            self.start_mods_update_check();
        }
        if open_dir {
            let dir = game_dir.join(kind.folder());
            let _ = std::fs::create_dir_all(&dir);
            open_folder(&dir);
        }
        if let Some(path) = reveal {
            reveal_file(&path);
        }
        for (entry, enabled) in toggles {
            let mut copy = entry.clone();
            copy.enabled = !enabled;
            match installed::set_enabled(&game_dir, &mut copy, enabled) {
                Ok(()) => {
                    self.content.notice = Some(format!(
                        "{} {}",
                        if enabled { "已启用" } else { "已禁用" },
                        entry.name
                    ));
                    self.content.error = None;
                    self.content.delete_confirm = None;
                    self.start_mods_scan();
                }
                Err(e) => self.content.error = Some(e.to_string()),
            }
        }
        for entry in deletes {
            if let Err(e) = rollback::backup_extra(&game_dir, "删除内容前", &loader_files) {
                self.content.error = Some(format!("无法创建恢复点: {e}"));
                break;
            }
            match installed::remove(&game_dir, &entry) {
                Ok(()) => {
                    self.content.notice = Some(format!("已删除 {}", entry.name));
                    self.content.error = None;
                    self.content.delete_confirm = None;
                    self.start_mods_scan();
                }
                Err(e) => self.content.error = Some(e.to_string()),
            }
        }
        if let Some(candidates) = update_all {
            self.review_update(candidates);
        } else if let Some(candidate) = update_one {
            self.review_update(vec![candidate]);
        }
        if go_browse {
            self.show_browse();
        }
        if open_rollback {
            self.open_rollback();
        }
    }
}

pub(crate) fn toggle_switch(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 18.0), egui::Sense::click());
    let value = if motion::reduced() {
        if on { 1.0 } else { 0.0 }
    } else {
        ui.ctx().animate_bool_with_time(response.id, on, 0.15)
    };
    ui.painter()
        .rect_filled(rect, 9, motion::mix(t::control(), t::accent(), value));
    ui.painter().circle_filled(
        egui::pos2(
            egui::lerp((rect.left() + 9.0)..=(rect.right() - 9.0), value),
            rect.center().y,
        ),
        6.0,
        t::on_accent(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(if on {
        "已启用 · 点击禁用"
    } else {
        "已禁用 · 点击启用"
    })
}

#[derive(Default)]
pub(crate) struct RowAction {
    pub(crate) toggle: Option<bool>,
    pub(crate) reveal: bool,
    pub(crate) ask_delete: bool,
    pub(crate) cancel_delete: bool,
    pub(crate) delete: bool,
    pub(crate) update: bool,
}

pub(crate) fn installed_row(
    ui: &mut egui::Ui,
    entry: &installed::InstalledEntry,
    size: Option<u64>,
    update: Option<&str>,
    confirming: bool,
    recent: bool,
    last: bool,
) -> RowAction {
    let mut action = RowAction::default();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, 10, t::wash());
    }
    let inner = rect.shrink2(egui::vec2(12.0, 6.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 10.0;
    if toggle_switch(&mut child, entry.enabled).clicked() {
        action.toggle = Some(!entry.enabled);
    }
    let actions_w = if confirming {
        130.0
    } else if update.is_some() {
        88.0
    } else {
        34.0
    };
    let text_w = (inner.width() - 36.0 - 20.0 - actions_w).max(140.0);
    child.allocate_ui_with_layout(
        egui::vec2(text_w, 44.0),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(3.0);
            let name_color = if entry.enabled { t::ink() } else { t::dim() };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short_text(&entry.name, 60))
                        .size(13.0)
                        .family(bold_family())
                        .color(name_color),
                )
                .truncate(),
            );
            let mut meta = Vec::new();
            if recent {
                meta.push("刚导入".to_string());
            }
            if !entry.enabled {
                meta.push("已禁用".to_string());
            }
            meta.push(entry.file.clone());
            if let Some(size) = size {
                meta.push(human_size(size));
            }
            if let Some(source) = entry.source {
                meta.push(source.label().to_string());
            }
            if let Some(version) = &entry.version_number {
                meta.push(version.clone());
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short_text(&meta.join(" · "), 92))
                        .size(13.0)
                        .color(if recent { t::accent() } else { t::sub() }),
                )
                .truncate(),
            );
        },
    );
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if confirming {
            if danger_tiny(ui, "确认删除").clicked() {
                action.delete = true;
            }
            if tiny_button(ui, "取消").clicked() {
                action.cancel_delete = true;
            }
        } else {
            let menu = egui::containers::menu::MenuButton::from_button(
                egui::Button::new(egui::RichText::new("⋯").size(15.0).color(t::sub()))
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(motion::R)
                    .min_size(egui::vec2(26.0, 24.0)),
            );
            let _ = menu.ui(ui, |ui| {
                ui.set_min_width(150.0);
                if ui.button("在文件夹中显示").clicked() {
                    action.reveal = true;
                    ui.close();
                }
                if ui
                    .button(egui::RichText::new("删除…").color(t::danger()))
                    .clicked()
                {
                    action.ask_delete = true;
                    ui.close();
                }
            });
            if let Some(version) = update
                && ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("可更新").size(13.0).color(t::accent()),
                        )
                        .fill(t::tint(t::warn(), 0.14))
                        .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.55)))
                        .corner_radius(motion::R)
                        .min_size(egui::vec2(52.0, 24.0)),
                    )
                    .on_hover_text(format!("更新到 {version}"))
                    .clicked()
            {
                action.update = true;
            }
        }
    });
    if !last {
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 14.0, rect.bottom()),
                egui::pos2(rect.right() - 14.0, rect.bottom()),
            ],
            egui::Stroke::new(1.0, t::hairline()),
        );
    }
    action
}

/// 状态筛选胶囊：「已禁用 3」。有颜色的数字提醒这里有事，点了就只看这些。
pub(crate) fn status_chip(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    selected: bool,
    tone: Option<egui::Color32>,
) -> egui::Response {
    let (fill, border) = if selected {
        (t::wash(), t::control())
    } else {
        (egui::Color32::TRANSPARENT, t::hairline())
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat::simple(
            egui::FontId::proportional(13.0),
            if selected { t::ink() } else { t::sub() },
        ),
    );
    job.append(
        &count.to_string(),
        6.0,
        egui::TextFormat::simple(
            egui::FontId::new(13.0, bold_family()),
            tone.filter(|_| count > 0)
                .unwrap_or(if selected { t::ink() } else { t::sub() }),
        ),
    );
    let response = ui.add(
        egui::Button::new(job)
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, border))
            .corner_radius(motion::R)
            .min_size(egui::vec2(0.0, 28.0)),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.on_hover_text(if selected {
        "再点一次看全部"
    } else {
        "只看这些"
    })
}
