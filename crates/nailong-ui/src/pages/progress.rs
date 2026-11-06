//! 安装和下载进行中的页面：现在在做哪一步、做完了哪几步、刚下载了哪些文件、从哪下、多快、还要多久。

use crate::prelude::*;

impl App {
    pub(crate) fn progress_view(&mut self, ui: &mut egui::Ui) {
        let fallback;
        let activity = match &self.activity {
            Some(activity) => activity,
            None => {
                fallback = Activity::new("", "准备中");
                &fallback
            }
        };
        let p = &activity.progress;
        let bench = matches!(self.busy, Some(Task::Bench));
        let verb = match self.busy {
            Some(Task::Mods) => "正在下载内容",
            Some(Task::Bench) => "正在测试",
            Some(Task::Java) => "正在下载 Java",
            _ => "正在安装",
        };
        let compact_h = ui.ctx().content_rect().height() < 680.0;
        let width = (ui.available_width() - 48.0).clamp(360.0, 640.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        let now = ui.input(|input| input.time);
        let shown = ui.ctx().animate_value_with_time(
            egui::Id::unique("progress_value"),
            p.fraction(),
            0.30,
        );
        let ring_h: f32 = if compact_h { 140.0 } else { 190.0 };
        let counts = if p.files_total > 0 {
            format!("{} / {} 个文件", p.files_done, p.files_total)
        } else {
            String::new()
        };
        let data = rings::Data {
            state: rings::State::Installing,
            fraction: if p.indeterminate { 0.0 } else { shown },
            missing: 0,
            progress_detail: counts.clone(),
            mods: self
                .home
                .cache
                .as_ref()
                .map(|cache| vec![rings::ModDot::default(); cache.mod_entries.len()])
                .unwrap_or_default(),
            update_names: Vec::new(),
            snapshots: self.home.cache.as_ref().map_or(0, |cache| cache.snapshots),
            latest_snapshot: self
                .home
                .cache
                .as_ref()
                .and_then(|cache| cache.latest_snapshot.map(human_age)),
            running_minutes: 0,
        };
        let mut cancel_clicked = false;
        ui.add_space(if compact_h { 8.0 } else { 20.0 });
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(width, ring_h), egui::Sense::hover());
                let ring_rect = egui::Rect::from_center_size(
                    rect.center(),
                    egui::vec2(ring_h * 300.0 / 260.0, ring_h),
                );
                let _ = rings::show(ui, ring_rect, &data);

                // 标题、这一步、百分比。
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let title = if activity.title.is_empty() {
                            verb.to_string()
                        } else {
                            format!("{verb} · {}", activity.title)
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(title).size(15.0).family(bold_family()),
                            )
                            .truncate(),
                        );
                        ui.label(egui::RichText::new(&p.stage).size(13.0).color(t::sub()));
                    });
                    if !p.indeterminate {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", shown * 100.0))
                                    .size(30.0)
                                    .family(light_family()),
                            );
                        });
                    }
                });
                ui.add_space(8.0);
                motion::progress_bar(ui, shown, p.indeterminate, now);
                ui.add_space(8.0);

                // 数量和速度。
                ui.horizontal(|ui| {
                    let mut left = Vec::new();
                    if !counts.is_empty() {
                        left.push(counts.clone());
                    }
                    if p.bytes_total > 0 {
                        left.push(progress_detail(p));
                    }
                    let left = if bench {
                        "后台跑模拟负载，不开游戏窗口；期间电脑会比较忙".to_string()
                    } else {
                        left.join(" · ")
                    };
                    ui.label(egui::RichText::new(left).size(13.0).color(t::sub()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut right = Vec::new();
                        if let Some(speed) = activity.speed() {
                            right.push(rate(speed));
                        }
                        if let Some(secs) = activity.eta() {
                            right.push(remaining(secs));
                        }
                        ui.label(
                            egui::RichText::new(right.join(" · "))
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });

                // 步骤和刚下载的文件。
                if !bench {
                    ui.add_space(14.0);
                    let top = ui.cursor().top();
                    ui.painter().line_segment(
                        [
                            egui::pos2(ui.min_rect().left(), top),
                            egui::pos2(ui.min_rect().left() + width, top),
                        ],
                        egui::Stroke::new(1.0, t::hairline()),
                    );
                    ui.add_space(12.0);
                    let two_columns = width >= 520.0;
                    let column = if two_columns {
                        (width - 32.0) / 2.0
                    } else {
                        width
                    };
                    let rows = if compact_h { 4 } else { 6 };
                    let steps = |ui: &mut egui::Ui| {
                        ui.set_width(column);
                        ui.label(
                            egui::RichText::new("步骤")
                                .size(13.0)
                                .family(bold_family())
                                .color(t::sub()),
                        );
                        ui.add_space(4.0);
                        let skip = activity.steps.len().saturating_sub(rows);
                        if skip > 0 {
                            ui.label(
                                egui::RichText::new(format!("…前面还有 {skip} 步"))
                                    .size(12.0)
                                    .color(t::dim()),
                            );
                        }
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for step in activity.steps.iter().skip(skip) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().interact_size.y = 20.0;
                                let (glyph, color) = match step.took {
                                    Some(_) => ("✓", t::ok()),
                                    None => ("•", t::accent()),
                                };
                                ui.label(egui::RichText::new(glyph).size(12.0).color(color));
                                ui.label(egui::RichText::new(&step.name).size(13.0).color(
                                    if step.took.is_some() {
                                        t::sub()
                                    } else {
                                        t::ink()
                                    },
                                ));
                                if let Some(d) = step.took {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(took(d))
                                                    .size(12.0)
                                                    .color(t::dim()),
                                            );
                                        },
                                    );
                                }
                            });
                        }
                    };
                    let files = |ui: &mut egui::Ui| {
                        ui.set_width(column);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("刚下载")
                                    .size(13.0)
                                    .family(bold_family())
                                    .color(t::sub()),
                            );
                            if let Some(source) = activity.source_label() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            egui::RichText::new(format!("来自 {source}"))
                                                .size(12.0)
                                                .color(t::dim()),
                                        )
                                        .on_hover_text(activity.source.as_deref().unwrap_or(""));
                                    },
                                );
                            }
                        });
                        ui.add_space(4.0);
                        if activity.recent.is_empty() {
                            ui.label(
                                egui::RichText::new("文件都已经在了，只核对一遍")
                                    .size(12.5)
                                    .color(t::dim()),
                            );
                        }
                        let font = egui::FontId::proportional(12.5);
                        for (index, name) in activity.recent.iter().take(rows + 1).enumerate() {
                            // 越早的越淡：一眼看出哪个是刚下好的。
                            let fade = 1.0 - index as f32 * 0.13;
                            let color = motion::mix(t::dim(), t::ink(), fade.clamp(0.0, 1.0) * 0.8);
                            let shown = elide_middle(ui, name, &font, column - 4.0);
                            ui.label(egui::RichText::new(shown).font(font.clone()).color(color))
                                .on_hover_text(name);
                        }
                    };
                    if two_columns {
                        ui.horizontal_top(|ui| {
                            ui.vertical(steps);
                            ui.add_space(24.0);
                            ui.vertical(files);
                        });
                    } else {
                        ui.vertical(steps);
                        ui.add_space(10.0);
                        ui.vertical(files);
                    }
                }

                ui.add_space(if compact_h { 12.0 } else { 18.0 });
                ui.vertical_centered(|ui| {
                    let label = match (self.cancel_requested, bench) {
                        (true, _) => "正在取消…",
                        (false, true) => "停止测试",
                        (false, false) => "取消",
                    };
                    if motion::secondary_sized(ui, label, !self.cancel_requested, 96.0, 34.0, 13.0)
                        .clicked()
                    {
                        cancel_clicked = true;
                    }
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(if bench {
                            "不碰任何版本、存档和设置；停止后之前的结论保持不变"
                        } else {
                            "已下载的部分会保留，下次只补缺失的文件"
                        })
                        .size(13.0)
                        .color(t::dim()),
                    );
                });
            });
        });
        if cancel_clicked {
            self.cancel_requested = true;
            cancel::cancel();
        }
    }
}
