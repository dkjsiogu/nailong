//! 存档：世界列表、快照时间线、恢复和自动备份。

use crate::prelude::*;

impl App {
    pub(crate) fn saves_game_dir(&self) -> Option<PathBuf> {
        let row = self.worlds.row.as_ref()?;
        Some(
            row.game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf()),
        )
    }

    pub(crate) fn open_saves(&mut self, row: Row) {
        if self.page_locked() {
            return;
        }
        self.selected = Some(row.id.clone());
        self.worlds.row = Some(row);
        self.show_saves = true;
        self.show_manage = false;
        self.show_mods = false;
        self.worlds.confirm = None;
        self.worlds.confirm_backup = None;
        self.worlds.notice = None;
        self.worlds.error = None;
        self.worlds.busy = false;
        self.worlds.rx = None;
        self.worlds.selected = None;
        self.reload_saves();
    }

    pub(crate) fn reload_saves(&mut self) {
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        match saves::worlds(&dir) {
            Ok(items) => {
                self.worlds.snapshots.clear();
                for world in &items {
                    let items = snapshots::list(&dir, &world.name).unwrap_or_default();
                    self.worlds.snapshots.insert(world.name.clone(), items);
                }
                if self
                    .worlds
                    .selected
                    .as_ref()
                    .is_none_or(|name| !items.iter().any(|world| &world.name == name))
                {
                    self.worlds.selected = items.first().map(|world| world.name.clone());
                }
                self.worlds.list = items;
            }
            Err(e) => self.worlds.error = Some(e.to_string()),
        }
    }

    pub(crate) fn start_snapshot(&mut self, world: String) {
        if self.worlds.busy {
            return;
        }
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        let keep = self.worlds.backup.keep;
        self.worlds.busy = true;
        self.worlds.auto_backup_running = false;
        self.worlds.error = None;
        let (tx, rx) = mpsc::channel();
        self.worlds.rx = Some(rx);
        std::thread::spawn(move || {
            let result = snapshots::backup(&dir, &world)
                .map(|snapshot| {
                    let _ = snapshots::prune(&dir, keep);
                    format!(
                        "已创建快照 {}（{}）",
                        snapshot.id,
                        human_size(snapshot.bytes)
                    )
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::SavesDone(result));
        });
    }

    pub(crate) fn start_restore(&mut self, world: String, id: String) {
        if self.worlds.busy {
            return;
        }
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        self.worlds.busy = true;
        self.worlds.auto_backup_running = false;
        self.worlds.error = None;
        let (tx, rx) = mpsc::channel();
        self.worlds.rx = Some(rx);
        std::thread::spawn(move || {
            let result = snapshots::restore(&dir, &world, &id)
                .map(|path| {
                    format!(
                        "已恢复到 {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::SavesDone(result));
        });
    }

    pub(crate) fn auto_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        for row in &self.rows {
            let dir = row
                .game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf());
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        if dirs.is_empty() {
            dirs.push(self.root.dir().to_path_buf());
        }
        dirs
    }

    pub(crate) fn maybe_auto_backup(&mut self) {
        if !self.worlds.backup.enabled || self.worlds.busy || self.busy.is_some() {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let due = self.worlds.backup.interval_minutes.max(1) * 60;
        if now.saturating_sub(self.worlds.backup.last_run) < due {
            return;
        }
        self.start_auto_backup();
    }

    pub(crate) fn start_auto_backup(&mut self) {
        let dirs = self.auto_dirs();
        let keep = self.worlds.backup.keep;
        self.worlds.busy = true;
        self.worlds.auto_backup_running = true;
        self.worlds.error = None;
        let (tx, rx) = mpsc::channel();
        self.worlds.rx = Some(rx);
        std::thread::spawn(move || {
            let mut count = 0usize;
            for dir in dirs {
                let Ok(worlds) = saves::worlds(&dir) else {
                    continue;
                };
                for world in worlds {
                    if snapshots::backup(&dir, &world.name).is_ok() {
                        count += 1;
                    }
                }
                let _ = snapshots::prune(&dir, keep);
            }
            let result = if count == 0 {
                Err("没有可备份的世界".to_string())
            } else {
                Ok(format!("自动备份完成：{count} 个世界"))
            };
            let _ = tx.send(Msg::SavesDone(result));
        });
    }

    /// 自动备份开关、间隔、保留份数与手动动作；窄窗口会放到标题下一行。
    pub(crate) fn saves_header_controls(
        &mut self,
        ui: &mut egui::Ui,
        backup_all: &mut bool,
        prune: &mut bool,
    ) {
        if quick_action(ui, "清理旧快照").clicked() {
            *prune = true;
        }
        ui.add_space(4.0);
        if quick_action(ui, "备份全部").clicked() {
            *backup_all = true;
        }
        ui.add_space(8.0);
        let mut keep = self.worlds.backup.keep;
        egui::ComboBox::from_id_salt("backup_keep")
            .selected_text(format!("保留 {keep} 份"))
            .width(92.0)
            .show_ui(ui, |ui| {
                for value in [5usize, 10, 20, 50] {
                    if ui
                        .selectable_label(keep == value, format!("保留 {value} 份"))
                        .clicked()
                    {
                        keep = value;
                    }
                }
            });
        if keep != self.worlds.backup.keep {
            self.worlds.backup.keep = keep;
            let _ = snapshots::save_config(&self.root, &self.worlds.backup);
        }
        let mut interval = self.worlds.backup.interval_minutes;
        egui::ComboBox::from_id_salt("backup_interval")
            .selected_text(format!("每 {interval} 分钟"))
            .width(96.0)
            .show_ui(ui, |ui| {
                for minutes in [15u64, 30, 60, 120] {
                    if ui
                        .selectable_label(interval == minutes, format!("每 {minutes} 分钟"))
                        .clicked()
                    {
                        interval = minutes;
                    }
                }
            });
        if interval != self.worlds.backup.interval_minutes {
            self.worlds.backup.interval_minutes = interval;
            let _ = snapshots::save_config(&self.root, &self.worlds.backup);
        }
        let mut enabled = self.worlds.backup.enabled;
        ui.horizontal(|ui| {
            if toggle_switch(ui, enabled).clicked() {
                enabled = !enabled;
            }
            ui.label(
                egui::RichText::new("自动备份")
                    .size(13.0)
                    .color(if enabled { t::ink() } else { t::sub() }),
            );
        });
        if enabled != self.worlds.backup.enabled {
            self.worlds.backup.enabled = enabled;
            let _ = snapshots::save_config(&self.root, &self.worlds.backup);
        }
    }

    pub(crate) fn saves_view(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self.worlds.row.clone() else {
            return;
        };
        let game_dir = self
            .saves_game_dir()
            .unwrap_or_else(|| self.root.dir().to_path_buf());
        let shared = row.game_dir.is_none();
        let mut reload = false;
        let mut open_dir: Option<PathBuf> = None;
        let mut backup_all = false;
        let mut prune = false;
        let mut chosen_world: Option<String> = None;
        let mut new_snapshot: Option<String> = None;
        let mut restore: Option<(String, String)> = None;
        let mut delete_world: Option<String> = None;
        let mut delete_snapshot: Option<(String, String)> = None;
        let mut wanted_row: Option<Row> = None;
        let version_rows: Vec<Row> = self.visible_rows().cloned().collect();

        egui::Panel::top("saves_header")
            .frame(
                egui::Frame::new()
                    .fill(t::bg())
                    .inner_margin(egui::Margin::symmetric(28, 14)),
            )
            .show(ui, |ui| {
                let wide = ui.available_width() >= 840.0;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("存档")
                                        .size(24.0)
                                        .family(display_family())
                                        .color(t::ink()),
                                );
                            });
                            ui.add_space(14.0);
                            egui::ComboBox::from_id_salt("saves_target")
                                .selected_text(
                                    egui::RichText::new(short_text(&row.label, 16))
                                        .size(13.0)
                                        .color(t::sub()),
                                )
                                .width(196.0)
                                .show_ui(ui, |ui| {
                                    for item in &version_rows {
                                        if ui
                                            .selectable_label(item.id == row.id, &item.label)
                                            .clicked()
                                        {
                                            wanted_row = Some(item.clone());
                                        }
                                    }
                                });
                        });
                        ui.label(
                            egui::RichText::new(if shared {
                                format!(
                                    "{} 个世界 · 该版本未隔离，存档在共享目录",
                                    self.worlds.list.len()
                                )
                            } else {
                                format!(
                                    "{} 个世界 · 快照分块去重，第二次只写改动块",
                                    self.worlds.list.len()
                                )
                            })
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                    if wide {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.saves_header_controls(ui, &mut backup_all, &mut prune);
                        });
                    }
                });
                if !wide {
                    ui.add_space(4.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 34.0),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            self.saves_header_controls(ui, &mut backup_all, &mut prune);
                        },
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(game_dir.display().to_string())
                            .size(13.0)
                            .color(t::sub()),
                    );
                    ui.add_space(8.0);
                    if action_link(ui, "打开目录").clicked() {
                        open_dir = Some(game_dir.clone());
                    }
                    if action_link(ui, "刷新").clicked() {
                        reload = true;
                    }
                    if self.worlds.busy {
                        busy_spinner(ui);
                        ui.label(
                            egui::RichText::new("正在写入快照…")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    }
                });
                if let Some(note) = &self.worlds.notice {
                    ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
                }
                if let Some(err) = &self.worlds.error {
                    ui.label(egui::RichText::new(err).size(13.0).color(t::warn()));
                }
            });

        egui::Panel::left("saves_worlds")
            .resizable(false)
            .exact_size(288.0)
            .frame(
                egui::Frame::new()
                    .fill(t::bg())
                    .inner_margin(egui::Margin::symmetric(14, 18)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("世界")
                            .size(15.0)
                            .family(display_family())
                            .color(t::ink()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{}", self.worlds.list.len()))
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let worlds = self.worlds.list.clone();
                        for world in &worlds {
                            let active =
                                self.worlds.selected.as_deref() == Some(world.name.as_str());
                            let snapshots = self
                                .worlds
                                .snapshots
                                .get(&world.name)
                                .map(|items| items.len())
                                .unwrap_or(0);
                            if save_world_row(ui, world, snapshots, active).clicked() {
                                chosen_world = Some(world.name.clone());
                            }
                        }
                        if worlds.is_empty() {
                            ui.add_space(16.0);
                            ui.label(egui::RichText::new("还没有存档").size(13.0).color(t::sub()));
                        }
                    });
            });

        let mut open_shots: Option<String> = None;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(24, 20)),
            )
            .show(ui, |ui| {
                if self.worlds.list.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(54.0);
                        egui::Frame::new()
                            .fill(t::surface())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(30, 28))
                            .show(ui, |ui| {
                                ui.set_width(360.0);
                                ui.label(
                                    egui::RichText::new("这个版本还没有存档")
                                        .size(15.0)
                                        .family(bold_family()),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "进游戏创建一个世界，回来刷新就能做快照和回滚。",
                                    )
                                    .size(13.0)
                                    .color(t::sub()),
                                );
                                ui.add_space(16.0);
                                if primary_wide(ui, "打开游戏目录", true, 180.0) {
                                    open_dir = Some(game_dir.clone());
                                }
                                ui.add_space(4.0);
                                if action_link(ui, "刷新").clicked() {
                                    reload = true;
                                }
                            });
                    });
                    return;
                }
                let Some(world) = self.worlds.selected.clone() else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(90.0);
                        ui.label(
                            egui::RichText::new("选择左边的世界查看快照")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                    return;
                };
                let snapshots = self
                    .worlds
                    .snapshots
                    .get(&world)
                    .cloned()
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("快照")
                                .size(24.0)
                                .family(display_family())
                                .color(t::ink()),
                        );
                    });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{} 份", snapshots.len()))
                            .size(13.0)
                            .color(t::dim()),
                    );
                    // 这个世界里截过的图：数字就是入口，点了去截图页看这一本相簿。
                    let shot_count = self.gallery.shots.as_ref().map_or(0, |shots| {
                        shots
                            .list
                            .iter()
                            .filter(|shot| {
                                shot.game_dir == game_dir
                                    && shot.meta.world.as_deref() == Some(world.as_str())
                            })
                            .count()
                    });
                    if shot_count > 0 {
                        ui.add_space(8.0);
                        if action_link(ui, &format!("{shot_count} 张截图 ›")).clicked() {
                            open_shots = Some(world.clone());
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::secondary_sized(
                            ui,
                            "立即快照",
                            !self.worlds.busy,
                            92.0,
                            32.0,
                            13.0,
                        )
                        .clicked()
                        {
                            new_snapshot = Some(world.clone());
                        }
                        if self.worlds.confirm.as_deref() == Some(world.as_str()) {
                            if ui.button("取消").clicked() {
                                self.worlds.confirm = None;
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("确认删除世界")
                                            .size(13.0)
                                            .color(t::danger()),
                                    )
                                    .fill(t::tint(t::danger(), 0.16))
                                    .stroke(egui::Stroke::new(1.0, t::tint(t::danger(), 0.71))),
                                )
                                .clicked()
                            {
                                delete_world = Some(world.clone());
                            }
                        } else if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("删除世界").size(13.0).color(t::sub()),
                                )
                                .fill(t::bg())
                                .stroke(egui::Stroke::new(1.0, t::hairline()))
                                .corner_radius(motion::R)
                                .min_size(egui::vec2(78.0, 28.0)),
                            )
                            .clicked()
                        {
                            self.worlds.confirm = Some(world.clone());
                        }
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if snapshots.is_empty() {
                            let (objects, bytes) = snapshots::repo_stats(&game_dir);
                            if saves_empty_card(
                                ui,
                                objects,
                                bytes,
                                self.worlds.backup.enabled,
                                self.worlds.backup.interval_minutes,
                                self.worlds.backup.keep,
                            ) {
                                new_snapshot = Some(world.clone());
                            }
                        }
                        if !snapshots.is_empty() {
                            egui::Frame::new()
                                .fill(t::surface())
                                .stroke(egui::Stroke::new(1.0, t::hairline()))
                                .corner_radius(motion::R)
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    for (index, snapshot) in snapshots.iter().enumerate() {
                                        let confirming = self.worlds.confirm_backup.as_deref()
                                            == Some(snapshot.id.as_str());
                                        let action = snapshot_row(
                                            ui,
                                            snapshot,
                                            confirming,
                                            index == 0,
                                            index + 1 == snapshots.len(),
                                        );
                                        if action.restore {
                                            restore = Some((world.clone(), snapshot.id.clone()));
                                        }
                                        if action.ask_delete {
                                            self.worlds.confirm_backup = Some(snapshot.id.clone());
                                        }
                                        if action.cancel_delete {
                                            self.worlds.confirm_backup = None;
                                        }
                                        if action.delete {
                                            delete_snapshot =
                                                Some((world.clone(), snapshot.id.clone()));
                                        }
                                    }
                                });
                        }
                    });
            });

        if let Some(dir) = open_dir {
            open_folder(&dir);
        }
        if reload {
            self.reload_saves();
        }
        if let Some(world) = chosen_world {
            self.worlds.selected = Some(world);
            self.worlds.confirm = None;
            self.worlds.confirm_backup = None;
        }
        if (new_snapshot.is_some() || restore.is_some() || delete_world.is_some() || backup_all)
            && let Some(reason) =
                self.write_block(self.worlds.row.as_ref().map(|row| row.id.as_str()))
        {
            self.worlds.notice = None;
            self.worlds.error = Some(if reason.starts_with("游戏") {
                "游戏正在使用这些世界，退出游戏后再备份或恢复".into()
            } else {
                reason.into()
            });
            new_snapshot = None;
            restore = None;
            delete_world = None;
            backup_all = false;
        }
        if let Some(world) = new_snapshot {
            self.start_snapshot(world);
        }
        if let Some((world, id)) = restore {
            self.start_restore(world, id);
        }
        if let Some(world) = delete_world {
            match saves::delete_world(&game_dir, &world) {
                Ok(()) => {
                    self.worlds.notice = Some(format!("已删除存档 {world}"));
                    self.worlds.confirm = None;
                    self.reload_saves();
                }
                Err(e) => self.worlds.error = Some(e.to_string()),
            }
        }
        if let Some((world, id)) = delete_snapshot {
            match snapshots::delete(&game_dir, &world, &id) {
                Ok(()) => {
                    self.worlds.confirm_backup = None;
                    self.reload_saves();
                }
                Err(e) => self.worlds.error = Some(e.to_string()),
            }
        }
        if backup_all {
            let dir = game_dir.clone();
            let keep = self.worlds.backup.keep;
            self.worlds.busy = true;
            self.worlds.auto_backup_running = false;
            let (tx, rx) = mpsc::channel();
            self.worlds.rx = Some(rx);
            std::thread::spawn(move || {
                let mut count = 0;
                if let Ok(worlds) = saves::worlds(&dir) {
                    for world in worlds {
                        if snapshots::backup(&dir, &world.name).is_ok() {
                            count += 1;
                        }
                    }
                }
                let _ = snapshots::prune(&dir, keep);
                let _ = tx.send(Msg::SavesDone(Ok(format!("已备份 {count} 个世界"))));
            });
        }
        if prune {
            match snapshots::prune(&game_dir, self.worlds.backup.keep) {
                Ok((removed, deleted)) => {
                    self.worlds.notice =
                        Some(format!("清理了 {removed} 份旧快照、{deleted} 个无用块"));
                    self.reload_saves();
                }
                Err(e) => self.worlds.error = Some(e.to_string()),
            }
        }
        if let Some(row) = wanted_row {
            self.selected = Some(row.id.clone());
            self.open_saves(row);
        }
    }
}

pub(crate) fn save_world_row(
    ui: &mut egui::Ui,
    world: &saves::World,
    snapshots: usize,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
    list_row_fill(ui, rect, active, response.hovered());
    let inner = rect.shrink2(egui::vec2(12.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(&world.name).size(14.0));
        ui.label(
            egui::RichText::new(format!(
                "{} · {} · {} 份快照",
                human_size(world.size),
                human_age(world.modified),
                snapshots
            ))
            .size(13.0)
            .color(t::sub()),
        );
    });
    response
}

#[derive(Default)]
pub(crate) struct SnapshotAction {
    pub(crate) restore: bool,
    pub(crate) ask_delete: bool,
    pub(crate) cancel_delete: bool,
    pub(crate) delete: bool,
}

pub(crate) fn snapshot_row(
    ui: &mut egui::Ui,
    snapshot: &snapshots::Snapshot,
    confirming: bool,
    newest: bool,
    last: bool,
) -> SnapshotAction {
    let mut action = SnapshotAction::default();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::hover());
    if response.hovered() {
        ui.painter().rect_filled(rect, 8, t::wash());
    }
    if newest {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 2.0, rect.top() + 12.0),
                egui::vec2(2.5, 28.0),
            ),
            1.0,
            t::accent(),
        );
    }
    let inner = rect.shrink2(egui::vec2(12.0, 6.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(
            egui::RichText::new(human_age(snapshot.created_at))
                .size(13.0)
                .family(bold_family()),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} 个文件 · {}",
                snapshot.files.len(),
                human_size(snapshot.bytes)
            ))
            .size(13.0)
            .color(t::sub()),
        );
    });
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if confirming {
            if danger_tiny(ui, "确认删除").clicked() {
                action.delete = true;
            }
            if tiny_button(ui, "取消").clicked() {
                action.cancel_delete = true;
            }
        } else {
            if tiny_button(ui, "删除").clicked() {
                action.ask_delete = true;
            }
            if small_accent(ui, "恢复").clicked() {
                action.restore = true;
            }
        }
    });
    if !last {
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 12.0, rect.bottom()),
                egui::pos2(rect.right() - 12.0, rect.bottom()),
            ],
            egui::Stroke::new(1.0, t::hairline()),
        );
    }
    action
}

pub(crate) fn saves_empty_card(
    ui: &mut egui::Ui,
    objects: usize,
    bytes: u64,
    auto: bool,
    interval: u64,
    keep: usize,
) -> bool {
    let mut create = false;
    ui.add_space(56.0);
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(1.0, t::hairline()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(22, 20))
        .show(ui, |ui| {
            let width = (ui.available_width() - 4.0).max(280.0);
            ui.set_width(width);
            let left = |ui: &mut egui::Ui| -> bool {
                ui.label(
                    egui::RichText::new("还没有快照")
                        .size(15.0)
                        .family(bold_family()),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("第一次会完整存一份，之后只写改动过的块。")
                            .size(13.0)
                            .color(t::sub()),
                    )
                    .wrap(),
                );
                ui.add_space(12.0);
                primary_wide(ui, "立即快照", true, 132.0)
            };
            let repo = |ui: &mut egui::Ui| {
                ui.label(egui::RichText::new("仓库").size(13.0).color(t::sub()));
                ui.add_space(3.0);
                ui.label(egui::RichText::new(human_size(bytes)).size(13.0));
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(format!("{objects} 个数据块 · backups/"))
                        .size(13.0)
                        .color(t::sub()),
                );
            };
            let policy = |ui: &mut egui::Ui| {
                ui.label(egui::RichText::new("备份策略").size(13.0).color(t::sub()));
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(if auto {
                        "自动备份已开启"
                    } else {
                        "自动备份已关闭"
                    })
                    .size(13.0),
                );
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(format!("每 {interval} 分钟 · 保留 {keep} 份"))
                        .size(13.0)
                        .color(t::sub()),
                );
            };
            if width >= 700.0 {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(320.0, 1.0),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.set_width(320.0);
                            create = left(ui);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(200.0, 1.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_width(200.0);
                                policy(ui);
                            },
                        );
                        ui.add_space(22.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(160.0, 1.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_width(160.0);
                                repo(ui);
                            },
                        );
                    });
                });
            } else {
                create = left(ui);
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(160.0, 1.0),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.set_width(160.0);
                            repo(ui);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(220.0, 1.0),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                ui.set_width(220.0);
                                policy(ui);
                            },
                        );
                    });
                });
            }
        });
    create
}

/// 存档页：世界、快照和自动备份。
pub(crate) struct WorldsState {
    pub(crate) row: Option<Row>,
    pub(crate) selected: Option<String>,
    pub(crate) list: Vec<saves::World>,
    pub(crate) confirm: Option<String>,
    pub(crate) confirm_backup: Option<String>,
    pub(crate) snapshots: HashMap<String, Vec<snapshots::Snapshot>>,
    pub(crate) rx: Option<mpsc::Receiver<Msg>>,
    pub(crate) busy: bool,
    pub(crate) notice: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) backup: snapshots::Config,
    pub(crate) auto_backup_running: bool,
}
