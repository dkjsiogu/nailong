//! 内容 › 项目详情：版本列表、前置依赖和下载。

use crate::prelude::*;

impl App {
    /// 一键安装：不选版本、不点详情，直接装最新兼容版 + 必需前置。
    pub(crate) fn quick_install(&mut self, project: mods::Project) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.content.row.clone() else {
            self.content.error = Some("先在右上角「安装到」选一个版本，再点 +".into());
            return;
        };
        let kind = self.content.kind;
        let loader = match (kind, mod_loader(row.loader)) {
            (mods::ContentKind::Mod, None) => {
                self.content.error = Some("原版不能装 Mod，先装 Fabric / NeoForge / Forge".into());
                return;
            }
            (_, Some(loader)) => loader,
            (_, None) => mods::Loader::Fabric,
        };
        let Some(game_dir) = self.content.game_dir.clone() else {
            self.content.error = Some("目标目录无效".into());
            return;
        };
        if row.instance.is_none() {
            if let Some(parent) = game_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                self.content.error = Some(format!("启用版本隔离失败: {e}"));
                return;
            }
        }
        cancel::reset();
        self.cancel_requested = false;
        self.content.quick = Some(project_key(&project));
        self.busy = Some(Task::Mods);
        self.content.error = None;
        self.content.notice = None;
        self.activity = Some(Activity::new(
            project.name.clone(),
            format!("安装 {}", project.name),
        ));
        let mc = row.mc.clone();
        let loader_files = loader_state_files(&self.root, &row, &game_dir);
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
            if let Err(e) = rollback::backup_extra(
                &game_dir,
                &format!("安装 {} 前", project.name),
                &loader_files,
            ) {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(Msg::ModsFailed(format!("无法创建恢复点: {e}")));
                }
                return;
            }
            let result = mods::install_project(&game_dir, kind, &project, &mc, loader, &sink);
            let msg = match result {
                Ok(report) => Msg::ModsDone(format!(
                    "{} 已安装（{} 个文件）",
                    project.name, report.downloaded
                )),
                Err(Error::Cancelled) => Msg::ModsCancelled,
                Err(e) => Msg::ModsFailed(e.to_string()),
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(msg);
            }
        });
    }

    pub(crate) fn select_mods_project(&mut self, project: mods::Project) {
        self.content.loading = false;
        self.content.browse_loading = false;
        self.content.selected = Some(project.clone());
        self.content.versions.clear();
        self.content.versions_loading = false;
        self.content.version = None;
        self.content.choices.clear();
        self.content.optional.clear();
        self.content.deps_loading = false;
        self.content.error = None;
        self.content.notice = None;
        let project_key = format!("{:?}:{}", project.source, project.id);
        let mc = self
            .content
            .mc
            .clone()
            .or_else(|| {
                self.install
                    .catalog
                    .as_ref()
                    .and_then(|catalog| catalog.minecraft.first().cloned())
            })
            .unwrap_or_else(|| "1.21.1".to_string());
        self.content.versions_loading = true;
        let loader = self.content.loader.unwrap_or(mods::Loader::Fabric);
        let (tx, rx) = mpsc::channel();
        self.content.rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::versions(&project, &mc, loader).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsVersions {
                project: project_key,
                result,
            });
        });
    }

    pub(crate) fn select_mods_version(&mut self, version: mods::Version) {
        let Some(mc) = self.content.mc.clone() else {
            return;
        };
        let loader = self.content.loader.unwrap_or(mods::Loader::Fabric);
        self.content.version = Some(version.clone());
        self.content.choices.clear();
        self.content.optional.clear();
        self.content.deps_loading = true;
        self.content.error = None;
        self.content.notice = None;
        let version_key = format!("{:?}:{}", version.source, version.id);
        let (tx, rx) = mpsc::channel();
        self.content.rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::dependency_choices(&version, &mc, loader).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsDeps {
                version: version_key,
                result,
            });
        });
    }

    pub(crate) fn start_mods_download(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.content.row.clone() else {
            self.content.error = Some("先在右上角「安装到」选一个版本，再安装内容".into());
            return;
        };
        let loader = mod_loader(row.loader).unwrap_or(mods::Loader::Fabric);
        if self.content.kind == mods::ContentKind::Mod && mod_loader(row.loader).is_none() {
            self.content.error = Some("原版不能安装 Mod，请切到资源包或光影".into());
            return;
        }
        let Some(version) = self.content.version.clone() else {
            self.content.error = Some("先选择一个 Mod 版本".into());
            return;
        };
        if self
            .content
            .choices
            .iter()
            .any(|choice| choice.dependency.kind == mods::DependencyKind::Incompatible)
        {
            self.content.error = Some("存在不兼容前置，不能下载".into());
            return;
        }
        let missing_optional = self.content.choices.iter().any(|choice| {
            choice.dependency.kind == mods::DependencyKind::Optional
                && self.content.optional.contains(&(
                    choice.dependency.source,
                    choice.dependency.project_id.clone(),
                ))
                && choice.version.is_none()
        });
        if missing_optional {
            self.content.error = Some("已选的可选前置没有兼容版本".into());
            return;
        }
        let Some(game_dir) = self.content.game_dir.clone() else {
            self.content.error = Some("目标目录无效".into());
            return;
        };
        let kind = self.content.kind;
        let mods_dir = game_dir.join(kind.folder());
        let rollback_dir = game_dir.clone();
        let rollback_reason = format!("安装 {} 前", version.file_name);
        let loader_files = loader_state_files(&self.root, &row, &game_dir);
        if let Err(e) = std::fs::create_dir_all(&mods_dir) {
            self.content.error = Some(format!("创建 mods 目录失败: {e}"));
            return;
        }
        if row.instance.is_none() {
            if let Some(parent) = game_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                self.content.error = Some(format!("启用版本隔离失败: {e}"));
                return;
            }
        }
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Mods);
        self.content.error = None;
        self.content.notice = None;
        self.activity = Some(Activity::new(
            version.file_name.clone(),
            format!("解析{}依赖", self.content.kind.label()),
        ));
        let optional = self.content.optional.clone();
        let mc = row.mc.clone();
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
            if let Err(e) = rollback::backup_extra(&rollback_dir, &rollback_reason, &loader_files) {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(Msg::ModsFailed(format!("无法创建恢复点: {e}")));
                }
                return;
            }
            let result = mods::plan(&version, &mc, loader, &optional).and_then(|plan| {
                let report = mods::download_plan(&mods_dir, &plan, false, &sink)?;
                if let Some(game_dir) = mods_dir.parent() {
                    installed::record_versions(game_dir, kind, &plan)?;
                }
                Ok(report)
            });
            let msg = match result {
                Ok(report) => Msg::ModsDone(format!(
                    "已下载 {} 个文件，跳过 {} 个已存在文件",
                    report.downloaded,
                    report.skipped.len()
                )),
                Err(Error::Cancelled) => Msg::ModsCancelled,
                Err(e) => Msg::ModsFailed(e.to_string()),
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(msg);
            }
        });
    }

    pub(crate) fn detail_view(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.content.selected.clone() else {
            return;
        };
        let key = project_key(&project);
        let installed = self.content.installed_keys.contains(&key);
        let versions = self.content.versions.clone();
        let selected = self.content.version.clone();
        let choices = self.content.choices.clone();
        let optional = self.content.optional.clone();
        let deps_loading = self.content.deps_loading;
        let versions_loading = self.content.versions_loading;
        let target = self.content.row.clone();
        let kind = self.content.kind;
        let mc = self.content.mc.clone();
        let loader = self.content.loader;
        let error = self.content.error.clone();
        let notice = self.content.notice.clone();
        let busy_reason = self.busy_reason();
        let busy = self.busy.is_some();
        let catalog = self
            .install
            .catalog
            .clone()
            .unwrap_or_else(catalog::Catalog::fallback);

        let mut back = false;
        let mut pick: Option<mods::Version> = None;
        let mut install = false;
        let mut wanted_mc: Option<String> = None;
        let mut wanted_loader: Option<Option<mods::Loader>> = None;
        let mut optional_toggle: Option<(mods::DependencyKey, bool)> = None;

        ui.horizontal(|ui| {
            if ghost_button(ui, "‹ 返回").clicked() {
                back = true;
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new("内容").size(13.0).color(t::sub()));
            ui.label(egui::RichText::new("/").size(13.0).color(t::sub()));
            ui.label(
                egui::RichText::new(short_text(&project.name, 26))
                    .size(13.0)
                    .color(t::sub()),
            );
        });
        if back {
            self.clear_mods_selection();
            return;
        }
        ui.add_space(12.0);

        let mut seen = BTreeSet::new();
        let categories: Vec<String> = project
            .categories
            .iter()
            .filter_map(|slug| {
                let (label, _) = mods::category_alias(slug, slug)?;
                Some(label.to_string())
            })
            .filter(|label| seen.insert(label.clone()))
            .take(4)
            .collect();

        card().show(ui, |ui| {
            let action_w = 322.0;
            let info_w = (ui.available_width() - action_w - 16.0).max(280.0);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(info_w, 1.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.set_width(info_w);
                        ui.horizontal(|ui| {
                            let icon_size = egui::vec2(60.0, 60.0);
                            if let Some(texture) = self.content.icons.get(&key) {
                                ui.add(
                                    egui::Image::from_texture(egui::load::SizedTexture::new(
                                        texture.id(),
                                        icon_size,
                                    ))
                                    .corner_radius(motion::R),
                                );
                            } else {
                                let (rect, _) =
                                    ui.allocate_exact_size(icon_size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, motion::R, t::wash());
                                let letter = project
                                    .name
                                    .chars()
                                    .next()
                                    .map(|c| c.to_uppercase().to_string())
                                    .unwrap_or_default();
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    letter,
                                    egui::FontId::proportional(24.0),
                                    t::sub(),
                                );
                            }
                            ui.add_space(13.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(&project.name)
                                            .size(24.0)
                                            .family(display_family())
                                            .color(t::ink()),
                                    );
                                    if installed {
                                        ui.add_space(3.0);
                                        egui::Frame::new()
                                            .fill(t::wash())
                                            .corner_radius(motion::R)
                                            .inner_margin(egui::Margin::symmetric(7, 2))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new("已安装")
                                                        .size(13.0)
                                                        .color(t::ok()),
                                                );
                                            });
                                    }
                                });
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {} · {} 次下载",
                                        project.source.label(),
                                        project.author,
                                        compact(project.downloads)
                                    ))
                                    .size(13.0)
                                    .color(t::sub()),
                                );
                            });
                        });
                        ui.add_space(11.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&project.summary)
                                    .size(13.0)
                                    .color(t::ink()),
                            )
                            .wrap(),
                        );
                        if !categories.is_empty() {
                            ui.add_space(9.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                for category in &categories {
                                    tag(ui, category, t::sub());
                                }
                            });
                        }
                    },
                );
                ui.add_space(14.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(action_w, 1.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.set_width(action_w);
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(action_w - 28.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("版本").size(13.0).color(t::sub()),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            match &target {
                                                Some(row) => ui.label(
                                                    egui::RichText::new(format!(
                                                        "安装到 {} · {}",
                                                        row.mc,
                                                        row.loader.name()
                                                    ))
                                                    .size(13.0)
                                                    .color(t::sub()),
                                                ),
                                                None => ui.label(
                                                    egui::RichText::new("只浏览")
                                                        .size(13.0)
                                                        .color(t::warn()),
                                                ),
                                            };
                                        },
                                    );
                                });
                                ui.add_space(4.0);
                                if mc.is_none() {
                                    ui.horizontal(|ui| {
                                        let mc_label =
                                            mc.clone().unwrap_or_else(|| "游戏版本".to_string());
                                        egui::ComboBox::from_id_salt("detail_mc")
                                            .selected_text(mc_label)
                                            .width(146.0)
                                            .show_ui(ui, |ui| {
                                                for version in catalog.minecraft.iter().take(80) {
                                                    if ui
                                                        .selectable_label(
                                                            mc.as_deref() == Some(version.as_str()),
                                                            version,
                                                        )
                                                        .clicked()
                                                    {
                                                        wanted_mc = Some(version.clone());
                                                    }
                                                }
                                            });
                                        if kind == mods::ContentKind::Mod {
                                            let loader_label = loader
                                                .map(|loader| loader.label().to_string())
                                                .unwrap_or_else(|| "加载器".to_string());
                                            egui::ComboBox::from_id_salt("detail_loader")
                                                .selected_text(loader_label)
                                                .width(104.0)
                                                .show_ui(ui, |ui| {
                                                    for candidate in [
                                                        mods::Loader::Fabric,
                                                        mods::Loader::NeoForge,
                                                        mods::Loader::Forge,
                                                        mods::Loader::Quilt,
                                                    ] {
                                                        if ui
                                                            .selectable_label(
                                                                loader == Some(candidate),
                                                                candidate.label(),
                                                            )
                                                            .clicked()
                                                        {
                                                            wanted_loader = Some(Some(candidate));
                                                        }
                                                    }
                                                });
                                        }
                                    });
                                    ui.add_space(6.0);
                                }
                                if versions_loading {
                                    ui.horizontal(|ui| {
                                        busy_spinner(ui);
                                        ui.label(
                                            egui::RichText::new("获取版本…")
                                                .size(13.0)
                                                .color(t::sub()),
                                        );
                                    });
                                } else if versions.is_empty() {
                                    ui.label(
                                        egui::RichText::new(if mc.is_none() {
                                            "选定游戏版本后获取文件"
                                        } else {
                                            "没有匹配当前版本的文件"
                                        })
                                        .size(13.0)
                                        .color(if mc.is_none() { t::sub() } else { t::warn() }),
                                    );
                                } else {
                                    let selected_text = selected
                                        .as_ref()
                                        .map(|v| v.version_number.clone())
                                        .unwrap_or_else(|| "选择版本".into());
                                    egui::ComboBox::from_id_salt("detail_version")
                                        .selected_text(selected_text)
                                        .width(ui.available_width())
                                        .show_ui(ui, |ui| {
                                            for version in &versions {
                                                if ui
                                                    .selectable_label(
                                                        selected
                                                            .as_ref()
                                                            .is_some_and(|v| v.id == version.id),
                                                        &version.version_number,
                                                    )
                                                    .clicked()
                                                {
                                                    pick = Some(version.clone());
                                                }
                                            }
                                        });
                                }

                                ui.add_space(10.0);
                                let incompatible = choices.iter().any(|choice| {
                                    choice.dependency.kind == mods::DependencyKind::Incompatible
                                });
                                let missing_required = choices.iter().any(|choice| {
                                    choice.dependency.kind == mods::DependencyKind::Required
                                        && choice.version.is_none()
                                });
                                let can_install = target.is_some()
                                    && selected.is_some()
                                    && !deps_loading
                                    && !incompatible
                                    && !missing_required
                                    && !busy;
                                let button_width = ui.available_width();
                                if motion::primary_sized_glyph(
                                    ui,
                                    if deps_loading {
                                        "解析前置…"
                                    } else {
                                        "安装"
                                    },
                                    can_install,
                                    button_width,
                                    40.0,
                                    13.5,
                                    motion::PrimaryGlyph::Download,
                                ) {
                                    install = true;
                                }
                                if busy {
                                    ui.add_space(5.0);
                                    ui.label(
                                        egui::RichText::new(busy_reason).size(13.0).color(t::dim()),
                                    );
                                }
                                if let Some(note) = &notice {
                                    ui.add_space(5.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(note).size(13.0).color(t::ok()),
                                        )
                                        .wrap(),
                                    );
                                }
                                if let Some(err) = &error {
                                    ui.add_space(5.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(err).size(13.0).color(t::danger()),
                                        )
                                        .wrap(),
                                    );
                                }
                                if target.is_none() {
                                    ui.add_space(6.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(
                                                "只浏览模式：在顶部「安装到」选一个版本后即可安装",
                                            )
                                            .size(13.0)
                                            .color(t::sub()),
                                        )
                                        .wrap(),
                                    );
                                } else if missing_required {
                                    ui.add_space(6.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new("缺少必需前置，暂时无法安装")
                                                .size(13.0)
                                                .color(t::warn()),
                                        )
                                        .wrap(),
                                    );
                                } else if incompatible {
                                    ui.add_space(6.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new("这个版本与当前环境不兼容")
                                                .size(13.0)
                                                .color(t::danger()),
                                        )
                                        .wrap(),
                                    );
                                }
                            });
                    },
                );
            });
        });

        ui.add_space(12.0);
        let left_w = ui.available_width() - 356.0 - 12.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_w, 1.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_width(left_w);
                    card().show(ui, |ui| {
                        ui.set_width(left_w - 36.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("版本信息")
                                    .size(15.0)
                                    .family(bold_family()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("选定版本后自动解析前置")
                                            .size(13.0)
                                            .color(t::sub()),
                                    );
                                },
                            );
                        });
                        ui.add_space(9.0);
                        if let Some(version) = &selected {
                            detail_kv(ui, "文件", &short_text(&version.file_name, 76));
                            ui.add_space(5.0);
                            let mut meta = Vec::new();
                            if let Some(size) = version.size {
                                meta.push(human_size(size));
                            }
                            if let Some(date) = &version.published {
                                meta.push(format!("发布于 {}", short_date(date)));
                            }
                            let size_line = if meta.is_empty() {
                                "—".to_string()
                            } else {
                                meta.join(" · ")
                            };
                            detail_kv(ui, "大小", &size_line);
                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                detail_kv_label(ui, "游戏版本");
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                                    for game in version.game_versions.iter().take(6) {
                                        tag(ui, game, t::ink());
                                    }
                                    if version.game_versions.len() > 6 {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "+{}",
                                                version.game_versions.len() - 6
                                            ))
                                            .size(13.0)
                                            .color(t::sub()),
                                        );
                                    }
                                });
                            });
                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                detail_kv_label(ui, "加载器");
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                                    for loader in version.loaders.iter().take(4) {
                                        tag(ui, &loader_label(loader), t::ink());
                                    }
                                });
                            });
                        } else {
                            ui.label(
                                egui::RichText::new("还没有选中版本")
                                    .size(13.0)
                                    .color(t::sub()),
                            );
                        }

                        ui.add_space(11.0);
                        ui.separator();
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("前置与依赖")
                                    .size(13.0)
                                    .family(bold_family()),
                            );
                            ui.add_space(5.0);
                            if !choices.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!("{} 项", choices.len()))
                                        .size(13.0)
                                        .color(t::sub()),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("必需前置自动安装")
                                            .size(13.0)
                                            .color(t::sub()),
                                    );
                                },
                            );
                        });
                        ui.add_space(8.0);
                        if selected.is_none() || versions_loading {
                            ui.label(
                                egui::RichText::new("选好版本后，这里会列出需要的前置。")
                                    .size(13.0)
                                    .color(t::sub()),
                            );
                        } else if deps_loading {
                            ui.horizontal(|ui| {
                                busy_spinner(ui);
                                ui.label(
                                    egui::RichText::new("解析前置…").size(13.0).color(t::sub()),
                                );
                            });
                        } else if choices.is_empty() {
                            ui.horizontal(|ui| {
                                status_dot(ui, true);
                                ui.label(
                                    egui::RichText::new("这个版本没有额外前置")
                                        .size(13.0)
                                        .color(t::sub()),
                                );
                            });
                        } else {
                            for (index, choice) in choices.iter().enumerate() {
                                let (label, color, fill) = match choice.dependency.kind {
                                    mods::DependencyKind::Required => {
                                        ("必需", t::accent(), t::tint(t::warn(), 0.14))
                                    }
                                    mods::DependencyKind::Optional => ("可选", t::sub(), t::wash()),
                                    mods::DependencyKind::Incompatible => {
                                        ("不兼容", t::danger(), t::tint(t::danger(), 0.14))
                                    }
                                    _ => ("内置", t::sub(), t::wash()),
                                };
                                ui.horizontal(|ui| {
                                    tag_fill(ui, label, color, fill);
                                    ui.add_space(4.0);
                                    let name = choice
                                        .project
                                        .as_ref()
                                        .map(|project| project.name.clone())
                                        .or_else(|| choice.dependency.name.clone())
                                        .unwrap_or_else(|| choice.dependency.project_id.clone());
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(short_text(&name, 44)).size(13.0),
                                        )
                                        .truncate(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if choice.dependency.kind
                                                == mods::DependencyKind::Optional
                                            {
                                                let key = (
                                                    choice.dependency.source,
                                                    choice.dependency.project_id.clone(),
                                                );
                                                let mut checked = optional.contains(&key);
                                                if ui
                                                    .checkbox(&mut checked, "")
                                                    .on_hover_text("一并安装这个可选前置")
                                                    .changed()
                                                {
                                                    optional_toggle = Some((key, checked));
                                                }
                                            }
                                            match &choice.version {
                                                Some(version) => ui.label(
                                                    egui::RichText::new(&version.version_number)
                                                        .size(13.0)
                                                        .color(t::sub()),
                                                ),
                                                None => ui.label(
                                                    egui::RichText::new("缺失")
                                                        .size(13.0)
                                                        .color(t::warn()),
                                                ),
                                            };
                                        },
                                    );
                                });
                                if index + 1 < choices.len() {
                                    ui.add_space(7.0);
                                }
                            }
                        }
                    });
                },
            );
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(
                egui::vec2(356.0, 1.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_width(356.0);
                    card().show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("版本记录")
                                    .size(15.0)
                                    .family(bold_family()),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(format!("{} 个", versions.len()))
                                    .size(13.0)
                                    .color(t::sub()),
                            );
                        });
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new("点一行切换要安装的版本")
                                .size(13.0)
                                .color(t::sub()),
                        );
                        ui.add_space(8.0);
                        if versions.is_empty() {
                            ui.label(
                                egui::RichText::new(if versions_loading {
                                    "正在获取…"
                                } else {
                                    "暂无版本"
                                })
                                .size(13.0)
                                .color(t::sub()),
                            );
                        }
                        for version in versions.iter().take(6) {
                            let active = selected
                                .as_ref()
                                .is_some_and(|current| current.id == version.id);
                            if version_row_mini(ui, version, active).clicked() {
                                pick = Some(version.clone());
                            }
                        }
                        if versions.len() > 6 {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "只列最近 6 个 · 共 {} 个版本",
                                    versions.len()
                                ))
                                .size(13.0)
                                .color(t::sub()),
                            );
                        }
                    });
                },
            );
        });

        if let Some(mc) = wanted_mc {
            self.content.mc = Some(mc);
            if self.content.loader.is_none() && kind == mods::ContentKind::Mod {
                self.content.loader = Some(mods::Loader::Fabric);
            }
            self.select_mods_project(project.clone());
        }
        if let Some(loader) = wanted_loader {
            self.content.loader = loader;
            self.select_mods_project(project.clone());
        }
        if let Some(version) = pick
            && selected.as_ref().map(|current| current.id.as_str()) != Some(version.id.as_str())
        {
            self.select_mods_version(version);
        }
        if let Some((key, checked)) = optional_toggle {
            if checked {
                self.content.optional.insert(key);
            } else {
                self.content.optional.remove(&key);
            }
        }
        if install {
            self.review_detail_install();
        }
    }
}

pub(crate) fn detail_kv_label(ui: &mut egui::Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(58.0, 16.0), egui::Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        t::sub(),
    );
}

pub(crate) fn detail_kv(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        detail_kv_label(ui, label);
        ui.add(egui::Label::new(egui::RichText::new(value).size(13.0).color(t::ink())).truncate());
    });
}

pub(crate) fn version_row_mini(
    ui: &mut egui::Ui,
    version: &mods::Version,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            8,
            if active {
                t::wash()
            } else {
                egui::Color32::TRANSPARENT
            },
        );
    }
    let inner = rect.shrink2(egui::vec2(9.0, 0.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    child.label(
        egui::RichText::new(short_text(&version.version_number, 34))
            .size(13.0)
            .family(bold_family())
            .color(t::ink()),
    );
    let mut meta = Vec::new();
    if let Some(size) = version.size {
        meta.push(human_size(size));
    }
    if let Some(date) = &version.published {
        meta.push(short_date(date));
    }
    let date = version
        .published
        .as_ref()
        .map(|date| short_date(date))
        .unwrap_or_default();
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new(date).size(13.0).color(t::sub()));
    });
    response.on_hover_text(format!("{} · {}", version.version_number, meta.join(" · ")))
}
