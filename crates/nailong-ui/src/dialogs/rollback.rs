//! 恢复点：Mod、配置和加载器回到之前的状态。

use crate::prelude::*;

impl App {
    pub(crate) fn reload_rollback_points(&mut self) {
        let Some(dir) = self.content.game_dir.clone() else {
            self.rollback.points.clear();
            return;
        };
        match rollback::list(&dir) {
            Ok(items) => {
                self.rollback.points = items;
                self.rollback.error = None;
            }
            Err(e) => self.rollback.error = Some(e.to_string()),
        }
    }

    pub(crate) fn open_rollback(&mut self) {
        if self.content.game_dir.is_none() {
            self.content.error = Some("先选一个安装目标，再查看恢复点".into());
            return;
        }
        self.rollback.open = true;
        self.rollback.confirm = None;
        self.rollback.notice = None;
        self.rollback.error = None;
        self.reload_rollback_points();
    }

    pub(crate) fn start_rollback_backup(&mut self) {
        if self.rollback.busy {
            return;
        }
        let Some(dir) = self.content.game_dir.clone() else {
            return;
        };
        self.rollback.busy = true;
        self.rollback.notice = None;
        self.rollback.error = None;
        let (tx, rx) = mpsc::channel();
        self.rollback.rx = Some(rx);
        let loader = self
            .content
            .row
            .as_ref()
            .map(|row| loader_state_files(&self.root, row, &dir))
            .unwrap_or_default();
        std::thread::spawn(move || {
            let result = rollback::backup_extra(&dir, "手动恢复点", &loader)
                .map(|point| {
                    format!(
                        "已创建恢复点（{} 个文件，{}）",
                        point.files.len(),
                        human_size(point.bytes)
                    )
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::RollbackDone(result));
        });
    }

    pub(crate) fn start_rollback_restore(&mut self, id: String) {
        if self.rollback.busy {
            return;
        }
        let Some(dir) = self.content.game_dir.clone() else {
            return;
        };
        self.rollback.busy = true;
        self.rollback.notice = None;
        self.rollback.error = None;
        self.rollback.confirm = None;
        let (tx, rx) = mpsc::channel();
        self.rollback.rx = Some(rx);
        std::thread::spawn(move || {
            let result = rollback::restore(&dir, &id)
                .map(|count| format!("已恢复 {count} 个文件；恢复前的状态也留了恢复点"))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::RollbackDone(result));
        });
    }

    /// 内容页的“恢复点”入口：恢复 Mod / 配置 / 内容索引，不动存档。
    pub(crate) fn rollback_overlay(&mut self, ctx: &egui::Context) {
        if !self.rollback.open {
            return;
        }
        let mut close = false;
        let mut create = false;
        let mut restore: Option<String> = None;
        let mut delete: Option<String> = None;
        let veil = t::veil(1.0);
        let modal = egui::Modal::new(egui::Id::unique("rollback_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("恢复点")
                                .size(24.0)
                                .family(display_family())
                                .color(t::ink()),
                        );
                        ui.label(
                            egui::RichText::new(
                                "恢复 Mod、配置、内容索引和加载器版本文件；恢复前会自动留一份当前状态",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("保留最近 {} 个", rollback::KEEP))
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
                ui.add_space(12.0);
                if let Some(note) = &self.rollback.notice {
                    ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
                    ui.add_space(6.0);
                }
                if let Some(err) = &self.rollback.error {
                    ui.label(egui::RichText::new(err).size(13.0).color(t::danger()));
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if self.rollback.busy {
                        busy_spinner(ui);
                        ui.label(egui::RichText::new("正在处理内容…").size(13.0).color(t::sub()));
                    } else if motion::primary_sized(ui, "立即创建恢复点", true, 128.0, 32.0, 11.0)
                    {
                        create = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::secondary_sized(ui, "完成", true, 68.0, 30.0, 11.0).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        if self.rollback.points.is_empty() {
                            ui.add_space(18.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("还没有恢复点")
                                        .size(15.0)
                                        .family(bold_family()),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "安装或更新内容前会自动创建一个；版本目录内的加载器 JSON / 补丁 jar 也会一起保存。",
                                    )
                                    .size(13.0)
                                    .color(t::sub()),
                                );
                            });
                            ui.add_space(18.0);
                        }
                        for point in &self.rollback.points {
                            egui::Frame::new()
                                .fill(t::bg())
                                .stroke(egui::Stroke::new(1.0, t::hairline()))
                                .corner_radius(motion::R)
                                .inner_margin(egui::Margin::symmetric(12, 9))
                                .show(ui, |ui| {
                                    ui.set_width(496.0);
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} · {}",
                                                    human_age(point.created_at),
                                                    point.reason
                                                ))
                                                .size(13.0)
                                                .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} 个文件 · {}",
                                                    point.files.len(),
                                                    human_size(point.bytes)
                                                ))
                                                .size(13.0)
                                                .color(t::sub()),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if self.rollback.confirm.as_deref()
                                                    == Some(point.id.as_str())
                                                {
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new("确认恢复")
                                                                    .size(13.0)
                                                                    .color(t::danger()),
                                                            )
                                                            .fill(
                                                                t::tint(t::danger(), 0.16),
                                                            )
                                                            .stroke(egui::Stroke::new(1.0, t::danger())),
                                                        )
                                                        .clicked()
                                                    {
                                                        restore = Some(point.id.clone());
                                                    }
                                                    if motion::secondary_sized(
                                                        ui, "取消", true, 52.0, 24.0, 10.0,
                                                    )
                                                    .clicked()
                                                    {
                                                        self.rollback.confirm = None;
                                                    }
                                                } else {
                                                    if motion::secondary_sized(
                                                        ui, "恢复", true, 56.0, 24.0, 10.0,
                                                    )
                                                    .clicked()
                                                    {
                                                        self.rollback.confirm =
                                                            Some(point.id.clone());
                                                    }
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new("删除")
                                                                    .size(13.0)
                                                                    .color(t::sub()),
                                                            )
                                                            .fill(egui::Color32::TRANSPARENT)
                                                            .stroke(egui::Stroke::NONE),
                                                        )
                                                        .clicked()
                                                    {
                                                        delete = Some(point.id.clone());
                                                    }
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(6.0);
                        }
                    });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("只恢复内容与加载器版本文件，不动世界存档；世界备份在「存档」里。")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if create {
            self.start_rollback_backup();
        }
        if let Some(id) = restore {
            self.start_rollback_restore(id);
        }
        if let Some(id) = delete
            && let Some(dir) = self.content.game_dir.clone()
        {
            match rollback::delete(&dir, &id) {
                Ok(()) => {
                    self.rollback.notice = Some("已删除恢复点".into());
                    self.reload_rollback_points();
                }
                Err(e) => self.rollback.error = Some(e.to_string()),
            }
        }
        if close {
            self.rollback.open = false;
        }
    }
}

/// 恢复点弹窗。
pub(crate) struct RollbackState {
    pub(crate) open: bool,
    pub(crate) points: Vec<rollback::Snapshot>,
    pub(crate) busy: bool,
    pub(crate) confirm: Option<String>,
    pub(crate) notice: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) rx: Option<mpsc::Receiver<Msg>>,
}
