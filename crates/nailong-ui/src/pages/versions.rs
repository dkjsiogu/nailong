//! 版本：已安装列表和详情、安装新版本、修复缺失的文件、导入整合包。

use crate::prelude::*;

impl App {
    pub(crate) fn open_install(&mut self) {
        self.show_install = true;
        self.show_manage = false;
        self.show_mods = false;
        self.show_saves = false;
        self.error = None;
        self.notice = None;
        self.install.repair_target = None;
        if self.install.catalog.is_some() {
            self.pick_defaults();
        } else {
            self.load_catalog();
        }
    }

    pub(crate) fn load_catalog(&mut self) {
        if self.install.catalog.is_some()
            || self.install.catalog_loading
            || self.install.catalog_rx.is_some()
            || self.busy.is_some()
        {
            return;
        }
        self.install.catalog_loading = true;
        self.install.catalog_error = None;
        let (tx, rx) = mpsc::channel();
        self.install.catalog_rx = Some(rx);
        let root = self.root.clone();
        let source = self.prefs.source.clone();
        std::thread::spawn(move || {
            let result = catalog::load(&root, &source).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Catalog(result));
        });
    }

    pub(crate) fn pick_defaults(&mut self) {
        let Some(cat) = self.install.catalog.clone() else {
            return;
        };
        let mc = if cat.minecraft.iter().any(|v| v == "1.21.1") {
            "1.21.1".to_string()
        } else {
            cat.minecraft.first().cloned().unwrap_or_default()
        };
        self.install.mc = mc.clone();
        self.install.kind = preferred_kind(&cat, &mc);
        self.install.fabric = preferred_fabric(&cat);
        self.install.neoforge = preferred_neoforge(&cat, &mc);
        self.install.forge = preferred_forge(&cat, &mc);
    }

    pub(crate) fn start_repair(&mut self, row: Row) {
        self.start_install_worker(row.mc, row.loader, row.loader_version.unwrap_or_default());
    }

    pub(crate) fn start_modpack(&mut self, path: PathBuf) {
        if self.busy.is_some() {
            return;
        }
        if !path.is_file() {
            self.error = Some(format!("找不到整合包: {}", path.display()));
            return;
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "整合包".to_string());
        self.error = None;
        self.notice = None;
        self.show_install = true;
        self.show_manage = false;
        self.show_mods = false;
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Install);
        self.activity = Some(Activity::new(name.clone(), "准备导入整合包"));
        self.install.repair_target = Some(RepairTarget::Modpack(path.clone()));
        self.log.push(format!("导入整合包 {name}"));

        let root = self.root.clone();
        let source = self.prefs.source.clone();
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
            let msg = match modpack::import_pack(&root, &path, &source, &sink) {
                Ok(instance) => Msg::InstallDone(format!("instance:{}", instance.id)),
                Err(Error::Cancelled) => Msg::InstallCancelled,
                Err(e) => Msg::InstallFailed(e.to_string()),
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(msg);
            }
        });
    }

    pub(crate) fn start_install_worker(&mut self, mc: String, loader: LoaderKind, version: String) {
        if self.busy.is_some() {
            return;
        }
        if loader != LoaderKind::Vanilla && version.is_empty() {
            self.error = Some(format!("这个版本没有可用的 {}", loader.name()));
            return;
        }
        let target = match loader {
            LoaderKind::Vanilla => mc.clone(),
            LoaderKind::Fabric => fabric::profile_id(&mc, &version),
            LoaderKind::NeoForge => format!("neoforge-{version}"),
            LoaderKind::Forge => forge::profile_id(&mc, &version),
        };
        let target_label = if loader == LoaderKind::Vanilla {
            mc.clone()
        } else {
            format!("{mc} · {} {version}", loader.name())
        };
        self.error = None;
        self.notice = None;
        self.show_install = true;
        self.show_manage = false;
        self.show_mods = false;
        if loader == LoaderKind::Vanilla {
            self.show_base = true;
        }
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Install);
        self.activity = Some(Activity::new(target_label.clone(), "准备安装"));
        self.install.repair_target = Some(RepairTarget::Loader {
            mc: mc.clone(),
            loader,
            version: version.clone(),
        });
        self.install.mc = mc.clone();
        self.install.kind = loader;
        match loader {
            LoaderKind::Fabric => self.install.fabric = version.clone(),
            LoaderKind::NeoForge => self.install.neoforge = version.clone(),
            LoaderKind::Forge => self.install.forge = version.clone(),
            LoaderKind::Vanilla => {}
        }
        self.log.push(format!("安装 {target_label}"));

        let root = self.root.clone();
        let source = self.prefs.source.clone();
        let point_dir = root.version_dir(&target);
        let point_files = vec![format!("{target}.json"), format!("{target}.jar")];
        let point_json = point_dir.join(format!("{target}.json"));
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
            // 重装/修复已有加载器前留一份，恢复点里会带上版本 JSON 与补丁 jar。
            if point_json.is_file()
                && let Err(e) = rollback::backup_extra(&point_dir, "更新加载器前", &point_files)
            {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(Msg::InstallFailed(format!("无法创建恢复点: {e}")));
                }
                return;
            }
            let result: Result<String, Error> = match loader {
                LoaderKind::Vanilla => {
                    vanilla::install_with(&root, &mc, &source, &sink).map(|()| target.clone())
                }
                LoaderKind::Fabric => fabric::install_with(&root, &mc, &version, &source, &sink),
                LoaderKind::NeoForge => {
                    neoforge::install_with(&root, &mc, &version, &source, &sink)
                }
                LoaderKind::Forge => forge::install_with(&root, &mc, &version, &source, &sink),
            };
            let msg = match result {
                Ok(id) => Msg::InstallDone(id),
                Err(Error::Cancelled) => Msg::InstallCancelled,
                Err(e) => Msg::InstallFailed(e.to_string()),
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(msg);
            }
        });
    }

    pub(crate) fn delete_row(&mut self, row: &Row) {
        if let Some(reason) = self.write_block(Some(&row.id)) {
            self.error = Some(reason.into());
            self.delete_confirm = None;
            return;
        }
        let result = if let Some(id) = row.id.strip_prefix("instance:") {
            self.root.remove_instance(id)
        } else {
            self.root.remove_version(&row.profile)
        };
        match result {
            Ok(()) => {
                self.notice = Some(format!("已删除 {}", row.label));
                self.error = None;
                self.delete_confirm = None;
                self.refresh();
                if self.visible_count() == 0 {
                    self.show_manage = false;
                    self.open_install();
                }
            }
            Err(e) => {
                self.error = Some(format!("删除失败: {e}"));
                self.delete_confirm = None;
            }
        }
    }

    pub(crate) fn version_actions(
        &mut self,
        ui: &mut egui::Ui,
        row: &Row,
        open_dir: &mut Option<PathBuf>,
    ) {
        if motion::primary_sized_glyph(
            ui,
            "启动",
            self.busy.is_none(),
            92.0,
            34.0,
            13.0,
            motion::PrimaryGlyph::Play,
        ) {
            self.selected = Some(row.id.clone());
            self.start_launch();
        }
        ui.add_space(6.0);
        if motion::secondary_sized(ui, "打开目录", true, 78.0, 34.0, 12.0).clicked() {
            *open_dir = Some(
                row.game_dir
                    .clone()
                    .unwrap_or_else(|| self.root.version_dir(&row.profile)),
            );
        }
    }

    pub(crate) fn manage_view(&mut self, ui: &mut egui::Ui) {
        let rows: Vec<Row> = self
            .rows
            .iter()
            .filter(|row| self.show_base || !row.hidden)
            .cloned()
            .collect();
        if self.selected.is_none()
            && let Some(first) = rows.first()
        {
            self.selected = Some(first.id.clone());
        }
        let selected = self.selected_row();
        let mut chosen: Option<Row> = None;
        let mut go_install = false;
        let mut delete_target: Option<Row> = None;
        let mut open_dir: Option<PathBuf> = None;

        egui::Panel::left("manage_versions")
            .resizable(false)
            .exact_size(292.0)
            .frame(
                egui::Frame::new()
                    .fill(t::bg())
                    .inner_margin(egui::Margin::symmetric(14, 18)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("版本")
                            .size(15.0)
                            .family(display_family())
                            .color(t::ink()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("＋").size(13.0).color(t::accent()),
                                )
                                .fill(t::bg())
                                .stroke(egui::Stroke::new(1.0, t::hairline()))
                                .corner_radius(motion::R)
                                .min_size(egui::vec2(24.0, 22.0)),
                            )
                            .on_hover_text("安装新版本")
                            .clicked()
                        {
                            go_install = true;
                        }
                        ui.label(
                            egui::RichText::new(format!("{}", rows.len()))
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
                ui.add_space(10.0);
                let mut show_base = self.show_base;
                ui.horizontal(|ui| {
                    if toggle_switch(ui, show_base).clicked() {
                        show_base = !show_base;
                    }
                    ui.label(
                        egui::RichText::new("显示基础版本")
                            .size(13.0)
                            .color(if show_base { t::ink() } else { t::sub() }),
                    );
                });
                if show_base != self.show_base {
                    self.show_base = show_base;
                    if !show_base {
                        self.ensure_visible_selection();
                    }
                }
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - 46.0).max(120.0))
                    .show(ui, |ui| {
                        for row in &rows {
                            let active = selected.as_ref().is_some_and(|item| item.id == row.id);
                            if version_row(ui, row, active).clicked() {
                                chosen = Some(row.clone());
                            }
                        }
                    });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(
                        egui::RichText::new("拖入 .mrpack / .zip 可直接导入整合包")
                            .size(13.0)
                            .color(t::dim()),
                    );
                    ui.add_space(4.0);
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(30, 26)),
            )
            .show(ui, |ui| {
                let Some(row) = selected.clone() else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(110.0);
                        ui.label(
                            egui::RichText::new("还没有安装任何版本")
                                .size(15.0)
                                .family(bold_family()),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("装一个原版或加载器版本，再回来管理内容与存档")
                                .size(13.0)
                                .color(t::sub()),
                        );
                        ui.add_space(16.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("安装新版本")
                                        .size(13.0)
                                        .family(bold_family())
                                        .color(t::ink()),
                                )
                                .fill(t::accent())
                                .corner_radius(motion::R)
                                .min_size(egui::vec2(150.0, 38.0)),
                            )
                            .clicked()
                        {
                            go_install = true;
                        }
                    });
                    return;
                };

                let compact = ui.available_width() < 800.0;
                ui.horizontal(|ui| {
                    let title_w = if compact {
                        ui.available_width()
                    } else {
                        (ui.available_width() - 196.0).max(240.0)
                    };
                    ui.allocate_ui_with_layout(
                        egui::vec2(title_w, 64.0),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    // 长名字换行完整显示，不截断。
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&row.label)
                                                .size(24.0)
                                                .family(display_family())
                                                .color(t::ink()),
                                        )
                                        .wrap(),
                                    );
                                });
                            });
                            ui.add_space(1.0);
                            ui.horizontal(|ui| {
                                status_dot(ui, row.health.missing == 0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {}",
                                        row.mc,
                                        row.loader.name()
                                    ))
                                    .size(13.0)
                                    .color(t::sub()),
                                );
                                ui.label(egui::RichText::new("·").size(13.0).color(t::sub()));
                                ui.label(
                                    egui::RichText::new(if row.health.missing == 0 {
                                        "安装完整".to_string()
                                    } else {
                                        format!("缺 {} 个文件", row.health.missing)
                                    })
                                    .size(13.0)
                                    .color(if row.health.missing == 0 { t::ok() } else { t::warn() }),
                                );
                                if row.health.missing > 0
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("补全").size(13.0),
                                            )
                                            .fill(t::bg())
                                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                                            .corner_radius(motion::R)
                                            .min_size(egui::vec2(40.0, 20.0)),
                                        )
                                        .clicked()
                                {
                                    self.selected = Some(row.id.clone());
                                    self.show_manage = false;
                                    self.start_repair(row.clone());
                                }
                            });
                        },
                    );
                    if !compact {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.version_actions(ui, &row, &mut open_dir);
                        });
                    }
                });
                if compact {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        self.version_actions(ui, &row, &mut open_dir);
                    });
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(16.0);
                        let (mods, packs, shaders, worlds) = self.version_summary(&row);
                        egui::Frame::new()
                            .fill(t::surface())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(18, 12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    stat_cell(ui, mods, "Mod");
                                    ui.add_space(14.0);
                                    stat_cell(ui, packs, "资源包");
                                    ui.add_space(14.0);
                                    stat_cell(ui, shaders, "光影");
                                    ui.add_space(14.0);
                                    stat_cell(ui, worlds, "存档");
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(
                                                        self.memory_label(&row),
                                                    )
                                                    .size(13.0)
                                                    .color(t::sub()),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Java 21 {}",
                                                        if self.env.java_ok {
                                                            "已检测到"
                                                        } else {
                                                            "未检测到"
                                                        }
                                                    ))
                                                    .size(13.0)
                                                    .color(if self.env.java_ok { t::ok() } else { t::warn() }),
                                                );
                                            });
                                        },
                                    );
                                });
                            });

                        ui.add_space(18.0);
                        ui.label(
                            egui::RichText::new("安装信息")
                                .size(15.0)
                                .family(bold_family())
                                .color(t::sub()),
                        );
                        ui.add_space(8.0);
                        egui::Frame::new()
                            .fill(t::surface())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(16, 14))
                            .show(ui, |ui| {
                                info_line(ui, "版本 ID", &row.id);
                                ui.add_space(8.0);
                                info_line(
                                    ui,
                                    "加载器",
                                    &match &row.loader_version {
                                        Some(version) => {
                                            format!("{} {}", row.loader.name(), version)
                                        }
                                        None => row.loader.name().to_string(),
                                    },
                                );
                                ui.add_space(8.0);
                                let dir = row
                                    .game_dir
                                    .clone()
                                    .unwrap_or_else(|| self.root.version_dir(&row.profile));
                                ui.horizontal(|ui| {
                                    if path_row(ui, "游戏目录", &dir) {
                                        open_dir = Some(dir.clone());
                                    }
                                });
                                ui.add_space(8.0);
                                if row.required_by > 0 {
                                    ui.add_space(8.0);
                                    info_line(ui, "被依赖", &format!("{} 个版本", row.required_by));
                                }
                                if row.used_by > 0 {
                                    ui.add_space(8.0);
                                    info_line(ui, "被使用", &format!("{} 个整合包", row.used_by));
                                }
                                ui.add_space(8.0);
                                info_line(
                                    ui,
                                    "隔离",
                                    if row.game_dir.is_some() {
                                        "已启用（内容与存档在版本目录）"
                                    } else {
                                        "未启用（与其它版本共享）"
                                    },
                                );
                            });

                        ui.add_space(20.0);
                        egui::Frame::new()
                            .fill(t::wash())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(16, 12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("内容与存档")
                                            .size(15.0)
                                            .family(bold_family()),
                                    );
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "在左侧「内容」「存档」里按版本管理，这里只负责启动与维护",
                                        )
                                        .size(13.0)
                                        .color(t::sub()),
                                    );
                                });
                            });

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if self.delete_confirm.as_deref() == Some(row.id.as_str()) {
                                ui.label(
                                    egui::RichText::new(format!("确认删除 {}？", row.label))
                                        .size(13.0)
                                        .color(t::warn()),
                                );
                                if ui.button("取消").clicked() {
                                    self.delete_confirm = None;
                                }
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("确认删除")
                                                .size(13.0)
                                                .color(t::danger()),
                                        )
                                        .fill(t::tint(t::danger(), 0.16))
                                        .stroke(egui::Stroke::new(
                                            1.0,
                                            t::tint(t::danger(), 0.71),
                                        )),
                                    )
                                    .clicked()
                                {
                                    delete_target = Some(row.clone());
                                }
                            } else if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("删除这个版本")
                                            .size(13.0)
                                            .color(t::danger()),
                                    )
                                    .fill(t::surface())
                                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                                    .corner_radius(motion::R)
                                    .min_size(egui::vec2(104.0, 30.0)),
                                )
                                .clicked()
                            {
                                self.delete_confirm = Some(row.id.clone());
                            }
                        });
                    });
            });

        if let Some(row) = chosen {
            self.selected = Some(row.id.clone());
            self.delete_confirm = None;
        }
        if let Some(dir) = open_dir {
            open_folder(&dir);
        }
        if let Some(row) = delete_target {
            self.delete_row(&row);
            self.delete_confirm = None;
        }
        if go_install {
            self.show_manage = false;
            self.open_install();
        }
    }

    pub(crate) fn version_summary(&mut self, row: &Row) -> (usize, usize, usize, usize) {
        if let Some((id, summary)) = &self.version_cache
            && id == &row.id
        {
            return *summary;
        }
        let summary = self.row_summary(row);
        self.version_cache = Some((row.id.clone(), summary));
        summary
    }

    pub(crate) fn row_summary(&self, row: &Row) -> (usize, usize, usize, usize) {
        let dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        let index = installed::load(&dir);
        let count = |kind: mods::ContentKind| {
            index
                .entries
                .iter()
                .filter(|entry| entry.kind == kind)
                .count()
        };
        (
            count(mods::ContentKind::Mod),
            count(mods::ContentKind::ResourcePack),
            count(mods::ContentKind::Shader),
            saves::worlds(&dir).map(|worlds| worlds.len()).unwrap_or(0),
        )
    }

    pub(crate) fn install_view(&mut self, ui: &mut egui::Ui) {
        if let Some(target) = self.install.repair_target.clone() {
            self.repair_view(ui, target);
            return;
        }
        let width = (ui.available_width() - 48.0).clamp(320.0, 1000.0);
        let pad = ((ui.available_width() - width) / 2.0).max(0.0);
        let gap = 20.0;
        let left_w = if width >= 1000.0 {
            588.0
        } else {
            (width - gap - 330.0).max(460.0)
        };
        let right_w = width - left_w - gap;
        let catalog = self
            .install
            .catalog
            .clone()
            .unwrap_or_else(catalog::Catalog::fallback);
        let mut mc = self.install.mc.clone();
        let mut kind = self.install.kind;
        let mut fv = self.install.fabric.clone();
        let mut nv = self.install.neoforge.clone();
        let mut gv = self.install.forge.clone();
        let mut go = false;
        let mut back = false;
        let mut choose_pack = false;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(36.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        let back_label = if self.rows.is_empty() {
                            "‹ 启动台"
                        } else {
                            "‹ 版本"
                        };
                        if ghost_button(ui, back_label).clicked() {
                            back = true;
                        }
                        ui.add_space(12.0);
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("安装游戏")
                                        .size(24.0)
                                        .family(display_family())
                                        .color(t::ink()),
                                );
                            });
                        });
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new("选择 Minecraft 版本和加载器，下载完整客户端")
                                .size(13.0)
                                .color(t::sub()),
                        );
                        ui.add_space(18.0);

                        ui.horizontal_top(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(left_w, 1.0),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    ui.set_width(left_w);
                                    egui::Frame::new()
                                        .fill(t::surface())
                                        .stroke(egui::Stroke::new(1.0, t::hairline()))
                                        .corner_radius(motion::R)
                                        .inner_margin(egui::Margin::symmetric(20, 18))
                                        .show(ui, |ui| {
                                            ui.set_width(left_w - 40.0);
                                            ui.label(
                                                egui::RichText::new("Minecraft 版本")
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                            ui.add_space(6.0);
                                            egui::ComboBox::from_id_salt("install_mc")
                                                .selected_text(mc.clone())
                                                .width(ui.available_width())
                                                .show_ui(ui, |ui| {
                                                    for v in &catalog.minecraft {
                                                        ui.selectable_value(&mut mc, v.clone(), v);
                                                    }
                                                });
                                            if mc != self.install.mc {
                                                self.install.mc = mc.clone();
                                                fv = preferred_fabric(&catalog);
                                                nv = preferred_neoforge(&catalog, &mc);
                                                gv = preferred_forge(&catalog, &mc);
                                                if !available(&catalog, kind, &mc) {
                                                    kind = preferred_kind(&catalog, &mc);
                                                }
                                            }
                                            ui.add_space(16.0);
                                            ui.label(
                                                egui::RichText::new("加载器")
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                            ui.add_space(6.0);
                                            let tile_w = ((ui.available_width() - 18.0) / 4.0)
                                                .clamp(96.0, 132.0);
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 6.0;
                                                for candidate in [
                                                    LoaderKind::Vanilla,
                                                    LoaderKind::Fabric,
                                                    LoaderKind::NeoForge,
                                                    LoaderKind::Forge,
                                                ] {
                                                    let enabled =
                                                        available(&catalog, candidate, &mc);
                                                    let subtitle = match candidate {
                                                        LoaderKind::Vanilla => "无加载器",
                                                        LoaderKind::Fabric => &fv,
                                                        LoaderKind::NeoForge => &nv,
                                                        LoaderKind::Forge => &gv,
                                                    };
                                                    if loader_tile(
                                                        ui,
                                                        candidate,
                                                        subtitle,
                                                        kind == candidate,
                                                        enabled,
                                                        tile_w,
                                                    )
                                                    .clicked()
                                                        && enabled
                                                    {
                                                        kind = candidate;
                                                    }
                                                }
                                            });
                                            if kind != LoaderKind::Vanilla {
                                                ui.add_space(10.0);
                                                let version = match kind {
                                                    LoaderKind::Fabric => &mut fv,
                                                    LoaderKind::NeoForge => &mut nv,
                                                    _ => &mut gv,
                                                };
                                                let values: Vec<String> = match kind {
                                                    LoaderKind::Fabric => catalog.fabric.clone(),
                                                    LoaderKind::NeoForge => catalog
                                                        .neoforge
                                                        .get(&mc)
                                                        .cloned()
                                                        .unwrap_or_default(),
                                                    _ => catalog
                                                        .forge
                                                        .get(&mc)
                                                        .cloned()
                                                        .unwrap_or_default(),
                                                };
                                                egui::ComboBox::from_id_salt("loader_version")
                                                    .selected_text(version.clone())
                                                    .width(200.0)
                                                    .show_ui(ui, |ui| {
                                                        for item in &values {
                                                            ui.selectable_value(
                                                                version,
                                                                item.clone(),
                                                                item,
                                                            );
                                                        }
                                                    });
                                            }
                                            ui.add_space(12.0);
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(ui.available_width(), 20.0),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    if self.install.catalog_loading {
                                                        busy_spinner(ui);
                                                        ui.label(
                                                            egui::RichText::new("获取版本…")
                                                                .size(13.0)
                                                                .color(t::sub()),
                                                        );
                                                    } else if let Some(err) =
                                                        &self.install.catalog_error
                                                    {
                                                        ui.label(
                                                            egui::RichText::new(err)
                                                                .size(13.0)
                                                                .color(t::sub()),
                                                        );
                                                    }
                                                },
                                            );
                                            ui.add_space(12.0);
                                            ui.separator();
                                            ui.add_space(8.0);
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "预计下载 200–300 MB · Java 21 {}",
                                                    if self.env.java_ok {
                                                        "已检测到"
                                                    } else {
                                                        "未检测到"
                                                    }
                                                ))
                                                .size(13.0)
                                                .color(t::sub()),
                                            );
                                        });
                                },
                            );
                            ui.add_space(gap);
                            ui.allocate_ui_with_layout(
                                egui::vec2(right_w, 1.0),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    ui.set_width(right_w);
                                    egui::Frame::new()
                                        .fill(t::surface())
                                        .stroke(egui::Stroke::new(1.0, t::hairline()))
                                        .corner_radius(motion::R)
                                        .inner_margin(egui::Margin::symmetric(18, 18))
                                        .show(ui, |ui| {
                                            ui.set_width(right_w - 36.0);
                                            ui.label(
                                                egui::RichText::new("将安装")
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                            ui.add_space(10.0);
                                            let version = match kind {
                                                LoaderKind::Vanilla => String::new(),
                                                LoaderKind::Fabric => fv.clone(),
                                                LoaderKind::NeoForge => nv.clone(),
                                                LoaderKind::Forge => gv.clone(),
                                            };
                                            info_line(ui, "Minecraft", &mc);
                                            ui.add_space(8.0);
                                            info_line(
                                                ui,
                                                "加载器",
                                                &match kind {
                                                    LoaderKind::Vanilla => "原版".to_string(),
                                                    _ => format!("{} {}", kind.name(), version),
                                                },
                                            );
                                            ui.add_space(8.0);
                                            info_line(ui, "下载", "约 200–300 MB");
                                            ui.add_space(8.0);
                                            info_line(
                                                ui,
                                                "目标",
                                                &self.root.dir().display().to_string(),
                                            );
                                            ui.add_space(14.0);
                                            ui.add_space(14.0);
                                            let can_install = self.busy.is_none()
                                                && !mc.is_empty()
                                                && available(&catalog, kind, &mc)
                                                && (kind == LoaderKind::Vanilla
                                                    || !version.is_empty());
                                            if motion::primary_sized_glyph(
                                                ui,
                                                "安装",
                                                can_install,
                                                ui.available_width(),
                                                40.0,
                                                14.0,
                                                motion::PrimaryGlyph::Download,
                                            ) {
                                                go = true;
                                            }
                                        });
                                },
                            );
                        });

                        ui.add_space(16.0);
                        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(
                                1.0,
                                if hovering { t::accent() } else { t::hairline() },
                            ))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(18, 12))
                            .show(ui, |ui| {
                                ui.set_width(width - 36.0);
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("导入整合包")
                                                .size(13.0)
                                                .family(bold_family()),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "拖入 .mrpack / .zip，自动安装加载器并解压",
                                            )
                                            .size(13.0)
                                            .color(t::sub()),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new("选择文件")
                                                        .fill(t::bg())
                                                        .stroke(egui::Stroke::new(
                                                            1.0,
                                                            t::hairline(),
                                                        ))
                                                        .corner_radius(motion::R)
                                                        .min_size(egui::vec2(86.0, 30.0)),
                                                )
                                                .clicked()
                                            {
                                                choose_pack = true;
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(14.0);
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(18, 14))
                            .show(ui, |ui| {
                                ui.set_width(width - 36.0);
                                ui.horizontal(|ui| {
                                    feature(ui, "断点续传", "下载中断后只补缺失部分");
                                    ui.add_space(28.0);
                                    feature(ui, "SHA1 校验", "每个文件落盘前校验");
                                    ui.add_space(28.0);
                                    feature(ui, "共享运行库", "多版本复用 libraries 与 assets");
                                });
                            });
                        if !self.env.java_ok {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("未检测到 Java 21，安装后无法启动")
                                    .size(13.0)
                                    .color(t::warn()),
                            );
                        }
                        ui.add_space(24.0);
                    });
                });
            });

        self.install.kind = kind;
        self.install.fabric = fv.clone();
        self.install.neoforge = nv.clone();
        self.install.forge = gv.clone();
        let version = match kind {
            LoaderKind::Vanilla => String::new(),
            LoaderKind::Fabric => fv.clone(),
            LoaderKind::NeoForge => nv.clone(),
            LoaderKind::Forge => gv.clone(),
        };
        if go {
            self.install.mc = mc.clone();
            self.start_install_worker(mc, kind, version);
        }
        if back {
            self.show_install = false;
            self.install.repair_target = None;
            self.error = None;
            if self.rows.is_empty() {
                self.show_manage = false;
            } else {
                self.show_manage = true;
                self.ensure_visible_selection();
            }
        }
        if choose_pack && let Some(path) = pick_modpack() {
            self.start_modpack(path);
        }
    }

    pub(crate) fn repair_view(&mut self, ui: &mut egui::Ui, target: RepairTarget) {
        let width = ui.available_width().min(760.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        ui.add_space(28.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.label(
                    egui::RichText::new("安装未完成")
                        .size(24.0)
                        .family(bold_family()),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(target.label())
                        .size(13.0)
                        .color(t::sub()),
                );
                ui.add_space(16.0);
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(20, 18))
                    .show(ui, |ui| {
                        if let Some(note) = &self.notice {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(note).size(13.0).color(t::ok()),
                                )
                                .wrap(),
                            );
                            ui.add_space(12.0);
                        }
                        if let Some(err) = &self.error {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(err).size(13.0).color(t::warn()),
                                )
                                .wrap(),
                            );
                            ui.add_space(12.0);
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    self.busy.is_none(),
                                    egui::Button::new(
                                        egui::RichText::new("继续安装")
                                            .size(13.0)
                                            .family(bold_family())
                                            .color(t::ink()),
                                    )
                                    .fill(t::accent())
                                    .corner_radius(motion::R)
                                    .min_size(egui::vec2(120.0, 36.0)),
                                )
                                .clicked()
                            {
                                match target.clone() {
                                    RepairTarget::Loader {
                                        mc,
                                        loader,
                                        version,
                                    } => self.start_install_worker(mc, loader, version),
                                    RepairTarget::Modpack(path) => self.start_modpack(path),
                                }
                            }
                            ui.add_space(8.0);
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("返回").size(13.0).color(t::sub()),
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::NONE),
                                )
                                .clicked()
                            {
                                self.show_install = false;
                                self.install.repair_target = None;
                                self.error = None;
                                self.show_manage = !self.rows.is_empty();
                                self.ensure_visible_selection();
                            }
                        });
                    });
            });
        });
    }
}

pub(crate) fn feature(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).size(13.0).family(bold_family()));
        ui.label(egui::RichText::new(detail).size(13.0).color(t::sub()));
    });
}

pub(crate) fn loader_tile(
    ui: &mut egui::Ui,
    kind: LoaderKind,
    subtitle: &str,
    selected: bool,
    enabled: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 58.0), egui::Sense::click());
    let hover = motion::hover_amount(ui, response.id.with("hover"), enabled && response.hovered());
    ui.painter().rect(
        rect,
        motion::R,
        if selected {
            t::wash()
        } else {
            t::tint(t::wash(), hover)
        },
        egui::Stroke::new(1.0, if selected { t::accent() } else { t::hairline() }),
        egui::StrokeKind::Inside,
    );
    let sub = if enabled && !subtitle.is_empty() {
        subtitle.to_string()
    } else if enabled {
        "内置".to_string()
    } else {
        "不可用".to_string()
    };
    ui.painter().text(
        rect.left_top() + egui::vec2(12.0, 12.0),
        egui::Align2::LEFT_TOP,
        kind.name(),
        egui::FontId::proportional(14.0),
        if enabled { t::ink() } else { t::dim() },
    );
    ui.painter().text(
        rect.left_top() + egui::vec2(12.0, 32.0),
        egui::Align2::LEFT_TOP,
        sub,
        egui::FontId::proportional(13.0),
        t::sub(),
    );
    response
}

pub(crate) fn version_row(ui: &mut egui::Ui, row: &Row, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
    list_row_fill(ui, rect, active, response.hovered());
    let inner = rect.shrink2(egui::vec2(12.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 10.0;
    status_dot(&mut child, row.health.missing == 0);
    child.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        // 列表里放不下就省略，悬停整行看全名。
        ui.add(egui::Label::new(egui::RichText::new(&row.label).size(14.0)).truncate());
        ui.label(
            egui::RichText::new(format!("{} · {}", row.mc, row.loader.name()))
                .size(13.0)
                .color(t::sub()),
        );
    });
    response.on_hover_text(&row.label)
}

/// 首次启动的步骤编号：完成的一步用绿色，未完成用暖色。
pub(crate) fn onboarding_badge(ui: &mut egui::Ui, number: &str, done: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
    let color = if done { t::ok() } else { t::sub() };
    ui.painter()
        .circle_stroke(rect.center(), 11.0, egui::Stroke::new(1.0, color));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if done { "✓" } else { number },
        egui::FontId::proportional(13.0),
        color,
    );
}

pub(crate) fn pick_modpack() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("选择整合包")
        .add_filter("整合包", &["mrpack", "zip"])
        .pick_file()
}

/// 安装新版本的表单和版本目录。
pub(crate) struct InstallForm {
    pub(crate) catalog: Option<catalog::Catalog>,
    pub(crate) catalog_loading: bool,
    pub(crate) catalog_error: Option<String>,
    pub(crate) catalog_rx: Option<mpsc::Receiver<Msg>>,
    pub(crate) mc: String,
    pub(crate) kind: LoaderKind,
    pub(crate) fabric: String,
    pub(crate) neoforge: String,
    pub(crate) forge: String,
    pub(crate) repair_target: Option<RepairTarget>,
}
