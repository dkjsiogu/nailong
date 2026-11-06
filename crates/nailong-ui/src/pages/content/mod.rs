//! 内容：发现与已安装两种视图共用的页头、安装目标和小工具。

use crate::prelude::*;

pub(crate) mod browse;
pub(crate) mod detail;
pub(crate) mod drop;
pub(crate) mod library;

impl App {
    #[cfg(debug_assertions)]
    pub(crate) fn open_mods(&mut self) {
        let row = self.selected_row();
        self.enter_mods(row);
    }

    pub(crate) fn enter_mods(&mut self, row: Option<Row>) {
        if self.page_locked() {
            return;
        }
        let game_dir = row.as_ref().map(|row| {
            row.game_dir
                .clone()
                .unwrap_or_else(|| self.root.version_dir(&row.profile))
        });
        let kind = match row.as_ref() {
            Some(row) if mod_loader(row.loader).is_none() => mods::ContentKind::ResourcePack,
            _ => mods::ContentKind::Mod,
        };
        self.content.mc = row.as_ref().map(|row| row.mc.clone());
        self.content.loader = row.as_ref().and_then(|row| mod_loader(row.loader));
        self.content.row = row;
        self.content.game_dir = game_dir;
        self.show_mods = true;
        self.show_install = false;
        self.show_manage = false;
        self.content.kind = kind;
        self.content.show_installed = false;
        self.content.installed.clear();
        self.content.installed_dir = None;
        self.content.installed_loading = false;
        self.content.updates_loading = false;
        self.content.updates.clear();
        self.content.query.clear();
        self.content.filter.clear();
        self.content.delete_confirm = None;
        self.content.results.clear();
        self.content.total = 0;
        self.content.warnings.clear();
        self.content.selected = None;
        self.content.versions.clear();
        self.content.versions_loading = false;
        self.content.version = None;
        self.content.choices.clear();
        self.content.optional.clear();
        self.content.notice = None;
        self.content.error = None;
        self.content.loading = false;
        self.content.deps_loading = false;
        self.content.rx = None;
        self.content.scan_rx = None;
        self.content.installed_loading = false;
        self.content.updates_loading = false;
        self.content.browse.clear();
        self.content.browse_loading = false;
        self.content.category = None;
        self.content.source_filter = None;
        self.content.chips_expanded = false;
        self.content.shelf = None;
        self.content.searching = false;
        self.content.quick = None;
        self.error = None;
        self.notice = None;
        self.refresh_installed_keys();
        self.fetch_cf_categories(self.content.kind);
        self.start_browse();
    }

    /// 只读本地索引，用来在卡片上显示“已装”。比在线扫描快，也不会占用消息通道。
    pub(crate) fn refresh_installed_keys(&mut self) {
        let Some(dir) = self.content.game_dir.clone() else {
            return;
        };
        let index = installed::load(&dir);
        self.content.installed_keys = index
            .entries
            .iter()
            .filter_map(|entry| {
                Some(format!(
                    "{:?}:{}",
                    entry.source?,
                    entry.project_id.as_ref()?
                ))
            })
            .collect();
    }

    /// 从项目详情回到浏览/搜索结果，保留列表与筛选。
    pub(crate) fn clear_mods_selection(&mut self) {
        self.content.selected = None;
        self.content.versions.clear();
        self.content.versions_loading = false;
        self.content.version = None;
        self.content.choices.clear();
        self.content.optional.clear();
        self.content.notice = None;
        self.content.error = None;
        self.content.deps_loading = false;
    }

    pub(crate) fn mods_view(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("mods_header_panel")
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin {
                        left: 28,
                        right: 28,
                        top: 16,
                        bottom: 10,
                    }),
            )
            .show(ui, |ui| {
                mods_column(ui, 1000.0, |ui| self.mods_header(ui));
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin {
                        left: 28,
                        right: 28,
                        top: 2,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        mods_column(ui, 1000.0, |ui| {
                            ui.add_space(14.0);
                            if self.content.show_installed {
                                self.installed_view(ui);
                            } else if self.content.selected.is_some() {
                                self.detail_view(ui);
                            } else if self.content.searching {
                                self.results_view(ui);
                            } else {
                                self.browse_view(ui);
                            }
                            ui.add_space(24.0);
                        });
                    });
            });
    }

    pub(crate) fn mods_header(&mut self, ui: &mut egui::Ui) {
        let installed_count = self
            .content
            .installed
            .iter()
            .filter(|entry| entry.kind == self.content.kind)
            .count();
        let mut wanted_target: Option<Option<Row>> = None;
        let mut search_go = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("内容")
                        .size(24.0)
                        .family(display_family())
                        .color(t::ink()),
                );
                ui.label(
                    egui::RichText::new("Mod、资源包、光影 · 已安装与发现")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let rows: Vec<Row> = self.visible_rows().cloned().collect();
                let label = self
                    .content
                    .row
                    .as_ref()
                    .map(|row| row.label.clone())
                    .unwrap_or_else(|| "只浏览".to_string());
                egui::ComboBox::from_id_salt("mods_target")
                    .selected_text(
                        egui::RichText::new(format!("安装到：{label}"))
                            .size(13.0)
                            .color(t::sub()),
                    )
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.content.row.is_none(), "只浏览（不安装）")
                            .clicked()
                        {
                            wanted_target = Some(None);
                        }
                        for row in &rows {
                            if ui
                                .selectable_label(
                                    self.content
                                        .row
                                        .as_ref()
                                        .is_some_and(|current| current.id == row.id),
                                    &row.label,
                                )
                                .clicked()
                            {
                                wanted_target = Some(Some(row.clone()));
                            }
                        }
                    });
                if !self.content.show_installed {
                    let go = motion::secondary_sized(ui, "搜索", true, 54.0, 30.0, 10.5).clicked();
                    let response = search_field(ui, &mut self.content.query, "名称或关键词", 200.0);
                    if std::mem::take(&mut self.content.focus_search) {
                        response.request_focus();
                    }
                    let enter =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if go || enter {
                        search_go = true;
                    }
                }
            });
        });
        if let Some(target) = wanted_target {
            self.set_mods_target(target);
        }
        if search_go {
            if self.content.query.trim().is_empty() {
                self.show_browse();
            } else {
                self.start_mods_search();
            }
        }

        ui.add_space(11.0);
        let mut mode: Option<bool> = None;
        let mut wanted_kind: Option<mods::ContentKind> = None;
        ui.horizontal(|ui| {
            if content_tab(ui, "已安装", installed_count, self.content.show_installed).clicked()
            {
                mode = Some(true);
            }
            ui.add_space(4.0);
            if content_tab(ui, "发现", 0, !self.content.show_installed).clicked() {
                mode = Some(false);
            }
            ui.add_space(14.0);
            for candidate in [
                mods::ContentKind::Mod,
                mods::ContentKind::ResourcePack,
                mods::ContentKind::Shader,
            ] {
                let selected = self.content.kind == candidate;
                let (fill, text, stroke) = if selected {
                    (t::tint(t::accent(), 0.17), t::ink(), t::accent())
                } else {
                    (egui::Color32::TRANSPARENT, t::sub(), t::hairline())
                };
                let button = egui::Button::new(
                    egui::RichText::new(candidate.label())
                        .size(13.0)
                        .family(bold_family())
                        .color(text),
                )
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .corner_radius(motion::R)
                .min_size(egui::vec2(72.0, 30.0));
                let tip = match candidate {
                    mods::ContentKind::Mod => "模组",
                    mods::ContentKind::ResourcePack => "材质与资源包",
                    mods::ContentKind::Shader => "光影",
                };
                if ui.add(button).on_hover_text(tip).clicked() && !selected {
                    wanted_kind = Some(candidate);
                }
            }
        });
        if let Some(installed) = mode {
            self.switch_content_mode(installed);
        }
        if let Some(kind) = wanted_kind {
            self.content.kind = kind;
            self.content.category = None;
            self.content.source_filter = None;
            self.content.shelf = None;
            self.content.searching = false;
            self.content.filter.clear();
            self.content.status = StatusFilter::All;
            self.content.delete_confirm = None;
            self.content.notice = None;
            self.content.error = None;
            self.clear_mods_selection();
            if self.content.show_installed {
                self.start_mods_scan();
            } else {
                self.start_browse();
            }
        }

        if !self.content.show_installed && self.content.row.is_none() {
            let mut wanted_mc: Option<Option<String>> = None;
            let mut wanted_loader: Option<Option<mods::Loader>> = None;
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let mc_label = self
                    .content
                    .mc
                    .clone()
                    .unwrap_or_else(|| "全部版本".to_string());
                egui::ComboBox::from_id_salt("browse_mc")
                    .selected_text(mc_label)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.content.mc.is_none(), "全部版本")
                            .clicked()
                        {
                            wanted_mc = Some(None);
                        }
                        let versions = self
                            .install
                            .catalog
                            .as_ref()
                            .map(|catalog| catalog.minecraft.clone())
                            .unwrap_or_else(|| vec!["1.21.1".to_string()]);
                        for version in versions.iter().take(80) {
                            if ui
                                .selectable_label(
                                    self.content.mc.as_deref() == Some(version.as_str()),
                                    version,
                                )
                                .clicked()
                            {
                                wanted_mc = Some(Some(version.clone()));
                            }
                        }
                    });
                if self.content.kind == mods::ContentKind::Mod {
                    let loader_label = self
                        .content
                        .loader
                        .map(|loader| loader.label().to_string())
                        .unwrap_or_else(|| "全部加载器".to_string());
                    egui::ComboBox::from_id_salt("browse_loader")
                        .selected_text(loader_label)
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.content.loader.is_none(), "全部加载器")
                                .clicked()
                            {
                                wanted_loader = Some(None);
                            }
                            for loader in [
                                mods::Loader::Fabric,
                                mods::Loader::NeoForge,
                                mods::Loader::Forge,
                                mods::Loader::Quilt,
                            ] {
                                if ui
                                    .selectable_label(
                                        self.content.loader == Some(loader),
                                        loader.label(),
                                    )
                                    .clicked()
                                {
                                    wanted_loader = Some(Some(loader));
                                }
                            }
                        });
                }
                ui.label(
                    egui::RichText::new("选一个目标版本可以一键安装到它；只浏览不会写入磁盘")
                        .size(13.0)
                        .color(t::sub()),
                );
            });
            if let Some(mc) = wanted_mc {
                self.content.mc = mc;
                if self.content.mc.is_none() {
                    self.content.loader = None;
                }
                self.reset_browse_selection();
                self.start_browse();
            }
            if let Some(loader) = wanted_loader
                && self.content.loader != loader
            {
                self.content.loader = loader;
                self.reset_browse_selection();
                self.start_browse();
            }
        }
    }

    /// 发现 / 已安装是同一个内容库的两种视图。
    pub(crate) fn switch_content_mode(&mut self, installed: bool) {
        if self.content.show_installed == installed {
            return;
        }
        self.content.show_installed = installed;
        self.content.filter.clear();
        self.content.delete_confirm = None;
        self.content.notice = None;
        self.content.error = None;
        self.clear_mods_selection();
        if installed {
            if self.content.game_dir.is_some() {
                self.start_mods_scan();
            } else {
                self.content.error = Some("先在上方选一个安装目标".into());
            }
        } else {
            self.show_browse();
        }
    }

    /// 切换下载中心的安装目标；None = 只浏览。
    pub(crate) fn set_mods_target(&mut self, row: Option<Row>) {
        if let Some(row) = &row {
            self.selected = Some(row.id.clone());
        }
        self.content.row = row.clone();
        self.content.game_dir = row.as_ref().map(|row| {
            row.game_dir
                .clone()
                .unwrap_or_else(|| self.root.version_dir(&row.profile))
        });
        match &row {
            Some(row) => {
                self.content.mc = Some(row.mc.clone());
                self.content.loader = mod_loader(row.loader);
            }
            None => {
                self.content.mc = None;
                self.content.loader = None;
            }
        }
        self.content.status = StatusFilter::All;
        self.content.recent.clear();
        self.content.selected = None;
        self.content.versions.clear();
        self.content.version = None;
        self.content.choices.clear();
        self.content.optional.clear();
        self.content.results.clear();
        self.content.total = 0;
        self.content.offset = 0;
        self.content.searching = false;
        self.content.error = None;
        self.content.notice = None;
        self.content.scan_rx = None;
        self.content.installed_loading = false;
        self.content.updates_loading = false;
        self.refresh_installed_keys();
        self.content.installed.clear();
        self.content.installed_dir = None;
        if self.content.show_installed {
            if self.content.game_dir.is_some() {
                self.start_mods_scan();
            }
        } else {
            self.fetch_cf_categories(self.content.kind);
            self.start_browse();
        }
    }
}

pub(crate) fn pick_content_files(kind: mods::ContentKind) -> Vec<PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(format!("选择{}文件（可多选）", kind.label()));
    match kind {
        mods::ContentKind::Mod => dialog.add_filter("Mod", &["jar"]),
        mods::ContentKind::ResourcePack | mods::ContentKind::Shader => {
            dialog.add_filter("内容包", &["zip", "jar"])
        }
    }
    .pick_files()
    .unwrap_or_default()
}

/// 内容接口的原始错误常带完整 URL；界面上只留一句可行动的话。
pub(crate) fn content_error(message: String) -> String {
    let text = message.trim();
    if text.starts_with("网络错误")
        || text.starts_with("请求失败")
        || text.contains("error sending request")
        || text.contains("dns error")
        || text.contains("timed out")
    {
        "网络连接失败，请检查网络或代理后重试".to_string()
    } else if text.starts_with("HTTP ") {
        "内容服务暂时不可用，请稍后重试".to_string()
    } else if text.contains("操作已取消") {
        "加载被中断，请重试".to_string()
    } else if text.chars().count() > 120 {
        format!("{}…", text.chars().take(120).collect::<String>())
    } else {
        text.to_string()
    }
}

/// 内容页统一列宽：宽窗口里居中，窄窗口里占满。
pub(crate) fn mods_column(ui: &mut egui::Ui, max_width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let available = ui.available_width();
    let width = available.min(max_width);
    let pad = ((available - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui);
        });
    });
}

pub(crate) fn content_ext(kind: mods::ContentKind) -> &'static str {
    match kind {
        mods::ContentKind::Mod => ".jar",
        mods::ContentKind::ResourcePack | mods::ContentKind::Shader => ".zip",
    }
}

pub(crate) fn content_size(
    game_dir: &std::path::Path,
    kind: mods::ContentKind,
    entry: &installed::InstalledEntry,
) -> Option<u64> {
    let file = if entry.enabled {
        entry.file.clone()
    } else {
        format!("{}.disabled", entry.file)
    };
    std::fs::metadata(game_dir.join(kind.folder()).join(file))
        .ok()
        .map(|metadata| metadata.len())
}

/// 内容页：发现和已安装两种视图共用的状态。
pub(crate) struct ContentState {
    pub(crate) row: Option<Row>,
    pub(crate) game_dir: Option<PathBuf>,
    pub(crate) query: String,
    pub(crate) filter: String,
    /// 已安装列表按状态筛：全部、已启用、已禁用、可更新、重复。和搜索可以叠加。
    pub(crate) status: StatusFilter,
    /// 这次导入的文件，列表里标「刚导入」，换页就清掉。
    pub(crate) recent: HashSet<String>,
    /// `mods_installed` 是哪个游戏目录的；换目录时先显示那个目录上次的索引。
    pub(crate) installed_dir: Option<PathBuf>,
    /// 内容目录上次扫描时的样子；窗口重新拿到焦点时比一比，变了就自动重扫。
    pub(crate) dir_sig: u64,
    pub(crate) delete_confirm: Option<String>,
    pub(crate) kind: mods::ContentKind,
    pub(crate) browse: Vec<Shelf>,
    pub(crate) browse_loading: bool,
    pub(crate) category: Option<mods::CategoryFilter>,
    pub(crate) source_filter: Option<mods::Source>,
    pub(crate) mc: Option<String>,
    pub(crate) loader: Option<mods::Loader>,
    pub(crate) chips_expanded: bool,
    pub(crate) shelf: Option<mods::Sort>,
    pub(crate) searching: bool,
    pub(crate) quick: Option<String>,
    pub(crate) installed_keys: HashSet<String>,
    pub(crate) show_installed: bool,
    pub(crate) installed_loading: bool,
    pub(crate) installed: Vec<installed::InstalledEntry>,
    pub(crate) updates_loading: bool,
    pub(crate) updates: Vec<installed::UpdateCandidate>,
    pub(crate) results: Vec<mods::Project>,
    pub(crate) total: u64,
    pub(crate) offset: usize,
    pub(crate) warnings: Vec<String>,
    pub(crate) loading: bool,
    pub(crate) selected: Option<mods::Project>,
    pub(crate) versions: Vec<mods::Version>,
    pub(crate) versions_loading: bool,
    pub(crate) version: Option<mods::Version>,
    pub(crate) choices: Vec<mods::DependencyChoice>,
    pub(crate) optional: BTreeSet<mods::DependencyKey>,
    pub(crate) deps_loading: bool,
    pub(crate) notice: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) rx: Option<mpsc::Receiver<Msg>>,
    /// 已安装扫描与更新检查单独走一条通道，避免覆盖浏览请求。
    pub(crate) scan_rx: Option<mpsc::Receiver<Msg>>,
    pub(crate) icons: HashMap<String, egui::TextureHandle>,
    pub(crate) icons_pending: HashSet<String>,
    pub(crate) cf_categories: HashMap<mods::ContentKind, Vec<mods::CurseCategory>>,
    pub(crate) cf_loading: HashSet<mods::ContentKind>,
    pub(crate) cf_rx: Option<mpsc::Receiver<CfCategoryFetch>>,
    pub(crate) icon_jobs: mpsc::Sender<(String, String)>,
    pub(crate) icon_rx: mpsc::Receiver<(String, Vec<u8>)>,
    /// 下一帧把焦点交给搜索框（Ctrl+F）。
    pub(crate) focus_search: bool,
    /// 拖着文件悬停时的预览：(这些文件, 说明, 能不能导入)。
    pub(crate) drop_preview: Option<(Vec<PathBuf>, String, bool)>,
}
