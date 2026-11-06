//! 安装和更新前的确认：装什么、带哪些前置、留不留恢复点。

use crate::prelude::*;

impl App {
    /// 安装确认：玩家点下安装之后、真正写盘之前，再看一眼清单。
    pub(crate) fn install_review_overlay(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_install.as_ref() else {
            return;
        };
        let review = pending.review.clone();
        let mut cancel = false;
        let mut confirm = false;
        let veil = t::veil(1.0);
        let modal = egui::Modal::new(egui::Id::unique("install_review_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(500.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("写入前先确认")
                            .size(13.0)
                            .color(t::accent()),
                        );
                        ui.label(
                            egui::RichText::new(if review.update {
                                "确认更新"
                            } else {
                                "确认安装"
                            })
                            .size(24.0)
                            .family(display_family())
                            .color(t::ink()),
                        );
                        ui.label(
                            egui::RichText::new(if review.update {
                                "先确认版本变化，再覆盖旧文件"
                            } else {
                                "先确认写入位置，再开始下载"
                            })
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let fill = t::tint(t::accent(), 0.16);
                        if review.update {
                            tag_pill(ui, "更新", t::accent(), fill);
                            ui.add_space(4.0);
                        }
                        tag_pill(ui, review.kind.label(), t::accent(), fill);
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(short_text(&review.name, 58))
                            .size(15.0)
                            .family(bold_family()),
                    );
                    if review.quick {
                        ui.label(
                            egui::RichText::new("一键安装")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    }
                });
                ui.add_space(9.0);
                egui::Frame::new()
                    .fill(t::bg())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(476.0);
                        install_review_row(
                            ui,
                            if review.update { "更新到" } else { "安装到" },
                            &review.target,
                        );
                        ui.add_space(5.0);
                        install_review_row(ui, "写入目录", &review.path);
                        ui.add_space(5.0);
                        install_review_row(
                            ui,
                            if review.update { "更新" } else { "版本" },
                            &review.version,
                        );
                        if let Some(size) = review.size {
                            ui.add_space(5.0);
                            install_review_row(ui, "预计大小", &human_size(size));
                        }
                        ui.add_space(5.0);
                        install_review_row(ui, "影响范围", "只写这个版本的内容目录，不动世界存档");
                    });
                ui.add_space(12.0);
                if review.update {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "将更新 {} 个项目",
                                review.updates.len()
                            ))
                            .size(13.0)
                            .family(bold_family()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new("旧文件会一并清理")
                                    .size(13.0)
                                    .color(t::sub()),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .id_salt("update_review_list")
                        .max_height(132.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for item in &review.updates {
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(short_text(&item.name, 34))
                                                .size(13.0),
                                        )
                                        .truncate(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(&item.to)
                                                    .size(13.0)
                                                    .color(t::accent()),
                                            );
                                            ui.label(
                                                egui::RichText::new("→").size(13.0).color(t::sub()),
                                            );
                                            ui.label(
                                                egui::RichText::new(short_text(&item.from, 18))
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                            }
                        });
                } else if review.quick {
                    ui.label(
                        egui::RichText::new(
                            "点「确认安装」后才会解析最新兼容版本与必需前置；解析失败不会改动现有文件。",
                        )
                        .size(13.0)
                        .color(t::sub()),
                    );
                } else if review.dependencies.is_empty() {
                    ui.horizontal(|ui| {
                        status_dot(ui, true);
                        ui.label(
                            egui::RichText::new("没有额外前置，只安装这一个文件")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "一起安装 {} 个前置",
                                review.dependencies.len()
                            ))
                            .size(13.0)
                            .family(bold_family()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new("必需自动勾选，可选按你的选择")
                                    .size(13.0)
                                    .color(t::sub()),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .id_salt("install_review_deps")
                        .max_height(112.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for dependency in &review.dependencies {
                                ui.horizontal(|ui| {
                                    if dependency.required {
                                        tag_fill(
                                            ui,
                                            "必需",
                                            t::accent(),
                                            t::tint(t::warn(), 0.14),
                                        );
                                    } else {
                                        tag_fill(ui, "可选", t::sub(), t::wash());
                                    }
                                    ui.add_space(3.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(short_text(
                                                &dependency.name,
                                                44,
                                            ))
                                            .size(13.0),
                                        )
                                        .truncate(),
                                    );
                                    if dependency.missing {
                                        ui.label(
                                            egui::RichText::new("缺失")
                                                .size(13.0)
                                                .color(t::warn()),
                                        );
                                    }
                                });
                                ui.add_space(4.0);
                            }
                        });
                }
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(t::tint(t::warn(), 0.09))
                    .stroke(egui::Stroke::new(
                        1.0,
                        t::tint(t::warn(), 0.35),
                    ))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(476.0);
                        ui.label(
                            egui::RichText::new(if review.update {
                                "更新前自动创建恢复点"
                            } else {
                                "安装前自动创建恢复点"
                            })
                            .size(13.0)
                            .family(bold_family())
                            .color(t::warn()),
                        );
                        ui.label(
                            egui::RichText::new(if review.update {
                                "更新后进不去，在「内容 › 恢复点」一键回到更新前。"
                            } else {
                                "装完发现不对，在「内容 › 恢复点」一键回到现在。"
                            })
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::primary_sized_glyph(
                            ui,
                            if review.update { "确认更新" } else { "确认安装" },
                            true,
                            118.0,
                            34.0,
                            12.0,
                            motion::PrimaryGlyph::Download,
                        ) {
                            confirm = true;
                        }
                        ui.add_space(8.0);
                        if motion::secondary_sized(ui, "取消", true, 76.0, 34.0, 11.5)
                            .clicked()
                        {
                            cancel = true;
                        }
                    });
                });
            });
        if modal.backdrop_response.clicked()
            || ctx.input(|input| input.key_pressed(egui::Key::Escape))
        {
            cancel = true;
        }
        if cancel {
            self.pending_install = None;
        }
        if confirm && let Some(pending) = self.pending_install.take() {
            match pending.request {
                InstallRequest::Quick(project) => self.quick_install(project),
                InstallRequest::Detail => self.start_mods_download(),
                InstallRequest::Update(candidates) => self.start_mods_update(candidates),
            }
        }
    }

    /// 一键安装前先把清单摆出来：装到哪、写什么、是否会动恢复点。
    pub(crate) fn review_quick_install(&mut self, project: mods::Project) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.content.row.clone() else {
            self.content.error = Some("先在右上角「安装到」选一个版本，再点 +".into());
            return;
        };
        if self.content.kind == mods::ContentKind::Mod && mod_loader(row.loader).is_none() {
            self.content.error = Some("原版不能装 Mod，先装 Fabric / NeoForge / Forge".into());
            return;
        }
        let Some(path) = self.content.game_dir.clone() else {
            self.content.error = Some("目标目录无效".into());
            return;
        };
        self.pending_install = Some(PendingInstall {
            review: InstallReview {
                name: project.name.clone(),
                kind: self.content.kind,
                target: target_label(&row),
                path: path.display().to_string(),
                version: "最新兼容版本 · 自动解析前置".into(),
                size: None,
                dependencies: Vec::new(),
                updates: Vec::new(),
                quick: true,
                update: false,
            },
            request: InstallRequest::Quick(project),
        });
    }

    /// 详情页的安装同样先确认，尤其是前置和可选依赖都选好之后。
    pub(crate) fn review_detail_install(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(project) = self.content.selected.clone() else {
            return;
        };
        let Some(version) = self.content.version.clone() else {
            return;
        };
        let Some(row) = self.content.row.clone() else {
            self.content.error = Some("先在「安装到」选一个版本".into());
            return;
        };
        let Some(path) = self.content.game_dir.clone() else {
            self.content.error = Some("目标目录无效".into());
            return;
        };
        let mut dependencies = Vec::new();
        let mut size = version.size.unwrap_or(0);
        for choice in &self.content.choices {
            let key = (
                choice.dependency.source,
                choice.dependency.project_id.clone(),
            );
            let required = choice.dependency.kind == mods::DependencyKind::Required;
            let optional = choice.dependency.kind == mods::DependencyKind::Optional;
            if !required && !(optional && self.content.optional.contains(&key)) {
                continue;
            }
            let name = choice
                .project
                .as_ref()
                .map(|project| project.name.clone())
                .or_else(|| choice.dependency.name.clone())
                .unwrap_or_else(|| choice.dependency.project_id.clone());
            if let Some(dep) = &choice.version {
                size += dep.size.unwrap_or(0);
            }
            dependencies.push(ReviewDependency {
                name,
                required,
                missing: choice.version.is_none(),
            });
        }
        self.pending_install = Some(PendingInstall {
            review: InstallReview {
                name: project.name.clone(),
                kind: self.content.kind,
                target: target_label(&row),
                path: path.display().to_string(),
                version: format!(
                    "{} · {}",
                    version.version_number,
                    short_text(&version.file_name, 42)
                ),
                size: (size > 0).then_some(size),
                dependencies,
                updates: Vec::new(),
                quick: false,
                update: false,
            },
            request: InstallRequest::Detail,
        });
    }

    /// 更新同样先确认：列出从哪个版本到哪个版本，别让“全部更新”变成盲盒。
    pub(crate) fn review_update(&mut self, candidates: Vec<installed::UpdateCandidate>) {
        if self.busy.is_some() || candidates.is_empty() {
            return;
        }
        let Some(row) = self.content.row.clone() else {
            self.content.error = Some("先在「安装到」选一个版本".into());
            return;
        };
        let Some(path) = self.content.game_dir.clone() else {
            self.content.error = Some("目标目录无效".into());
            return;
        };
        let mut updates = Vec::new();
        let mut size = 0u64;
        for candidate in &candidates {
            let from = candidate
                .entry
                .version_number
                .clone()
                .unwrap_or_else(|| "已装版本".to_string());
            let to = candidate.latest.version_number.clone();
            size += candidate.latest.size.unwrap_or(0);
            updates.push(ReviewUpdate {
                name: candidate.entry.name.clone(),
                from,
                to,
            });
        }
        let name = if updates.len() == 1 {
            updates[0].name.clone()
        } else {
            "内容更新".to_string()
        };
        self.pending_install = Some(PendingInstall {
            review: InstallReview {
                name,
                kind: self.content.kind,
                target: target_label(&row),
                path: path.display().to_string(),
                version: format!("{} 个项目", updates.len()),
                size: (size > 0).then_some(size),
                dependencies: Vec::new(),
                updates,
                quick: false,
                update: true,
            },
            request: InstallRequest::Update(candidates),
        });
    }
}

/// 安装确认里的一行「标签 + 值」。长路径省略显示，悬停可看全。
pub(crate) fn install_review_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [64.0, 16.0],
            egui::Label::new(egui::RichText::new(label).size(13.0).color(t::sub())),
        );
        ui.add(egui::Label::new(egui::RichText::new(value).size(13.0)).truncate())
            .on_hover_text(value);
    });
}
