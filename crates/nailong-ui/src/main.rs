#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::auth;
use nailong_core::install::{catalog, fabric, forge, health, modpack, neoforge, vanilla};
use nailong_core::installed;
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::mods;
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::saves;
use nailong_core::{Error, Root, cancel, java};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(242, 193, 78);
const CARD: egui::Color32 = egui::Color32::from_rgb(24, 27, 33);
const PANEL: egui::Color32 = egui::Color32::from_rgb(20, 22, 27);
const LINE: egui::Color32 = egui::Color32::from_rgb(42, 46, 56);
const MUTED: egui::Color32 = egui::Color32::from_rgb(145, 148, 156);
const WARN: egui::Color32 = egui::Color32::from_rgb(242, 160, 90);
const OK: egui::Color32 = egui::Color32::from_rgb(120, 190, 140);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LoaderKind {
    Vanilla,
    Fabric,
    NeoForge,
    Forge,
}

impl LoaderKind {
    fn name(self) -> &'static str {
        match self {
            Self::Vanilla => "原版",
            Self::Fabric => "Fabric",
            Self::NeoForge => "NeoForge",
            Self::Forge => "Forge",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Fabric => 0,
            Self::NeoForge => 1,
            Self::Forge => 2,
            Self::Vanilla => 3,
        }
    }
}

fn mod_loader(kind: LoaderKind) -> Option<mods::Loader> {
    match kind {
        LoaderKind::Fabric => Some(mods::Loader::Fabric),
        LoaderKind::NeoForge => Some(mods::Loader::NeoForge),
        LoaderKind::Forge => Some(mods::Loader::Forge),
        LoaderKind::Vanilla => None,
    }
}

#[derive(Clone)]
struct Row {
    id: String,
    profile: String,
    label: String,
    mc: String,
    loader: LoaderKind,
    loader_version: Option<String>,
    game_dir: Option<PathBuf>,
    instance: Option<String>,
    hidden: bool,
    required_by: usize,
    used_by: usize,
    health: health::Health,
}

#[derive(Clone)]
enum RepairTarget {
    Loader {
        mc: String,
        loader: LoaderKind,
        version: String,
    },
    Modpack(PathBuf),
}

impl RepairTarget {
    fn label(&self) -> String {
        match self {
            Self::Loader {
                mc,
                loader,
                version,
            } => {
                if *loader == LoaderKind::Vanilla {
                    mc.clone()
                } else {
                    format!("{mc} · {} {version}", loader.name())
                }
            }
            Self::Modpack(path) => path
                .file_name()
                .map(|s| format!("整合包 {}", s.to_string_lossy()))
                .unwrap_or_else(|| "整合包".to_string()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Task {
    Install,
    Mods,
    Launch,
}

enum Msg {
    Progress(Progress),
    Catalog(Result<catalog::Catalog, String>),
    InstallDone(String),
    InstallFailed(String),
    InstallCancelled,
    ModsSearch {
        query: String,
        offset: usize,
        result: Result<mods::SearchPage, String>,
    },
    ModsVersions {
        project: String,
        result: Result<Vec<mods::Version>, String>,
    },
    ModsDeps {
        version: String,
        result: Result<Vec<mods::DependencyChoice>, String>,
    },
    ModsDone(String),
    ModsFailed(String),
    ModsCancelled,
    ModsInstalled(Result<Vec<installed::InstalledEntry>, String>),
    ModsUpdates(Result<Vec<installed::UpdateCandidate>, String>),
    Browse {
        kind: mods::ContentKind,
        category: Option<mods::CategoryFilter>,
        source: Option<mods::Source>,
        result: Result<Vec<Shelf>, String>,
    },
    BrowseMore {
        sort: mods::Sort,
        offset: usize,
        result: Result<mods::SearchPage, String>,
    },
    SavesDone(Result<String, String>),
    LoginBrowser(String),
    LoginDone(Result<auth::Account, String>),
    LaunchDone(i32),
    LaunchFailed(String),
}

/// 浏览页的一排内容：热门 / 最新。
struct Shelf {
    title: &'static str,
    sort: mods::Sort,
    projects: Vec<mods::Project>,
}

type CfCategoryFetch = (mods::ContentKind, Result<Vec<mods::CurseCategory>, String>);

fn project_key(project: &mods::Project) -> String {
    format!("{:?}:{}", project.source, project.id)
}

struct App {
    root: Root,
    rows: Vec<Row>,
    selected: Option<String>,
    username: String,
    memory: u32,
    java_ok: bool,
    show_install: bool,
    show_manage: bool,
    show_mods: bool,
    show_base: bool,
    delete_confirm: Option<String>,
    mods_row: Option<Row>,
    mods_game_dir: Option<PathBuf>,
    mods_query: String,
    mods_kind: mods::ContentKind,
    mods_browse: Vec<Shelf>,
    mods_browse_loading: bool,
    mods_category: Option<mods::CategoryFilter>,
    mods_source_filter: Option<mods::Source>,
    mods_mc: Option<String>,
    mods_loader: Option<mods::Loader>,
    mods_chips_expanded: bool,
    cf_categories: HashMap<mods::ContentKind, Vec<mods::CurseCategory>>,
    cf_loading: HashSet<mods::ContentKind>,
    cf_rx: Option<mpsc::Receiver<CfCategoryFetch>>,
    mods_shelf: Option<mods::Sort>,
    mods_searching: bool,
    mods_quick: Option<String>,
    mods_icons: HashMap<String, egui::TextureHandle>,
    mods_icons_pending: HashSet<String>,
    icon_jobs: mpsc::Sender<(String, String)>,
    icon_rx: mpsc::Receiver<(String, Vec<u8>)>,
    mods_installed_keys: HashSet<String>,
    mods_show_installed: bool,
    mods_installed_loading: bool,
    mods_installed: Vec<installed::InstalledEntry>,
    mods_updates_loading: bool,
    mods_updates: Vec<installed::UpdateCandidate>,
    mods_results: Vec<mods::Project>,
    mods_total: u64,
    mods_offset: usize,
    mods_warnings: Vec<String>,
    mods_loading: bool,
    mods_selected: Option<mods::Project>,
    mods_versions: Vec<mods::Version>,
    mods_version: Option<mods::Version>,
    mods_choices: Vec<mods::DependencyChoice>,
    mods_optional: BTreeSet<mods::DependencyKey>,
    mods_deps_loading: bool,
    mods_notice: Option<String>,
    mods_error: Option<String>,
    mods_rx: Option<mpsc::Receiver<Msg>>,
    catalog: Option<catalog::Catalog>,
    catalog_loading: bool,
    catalog_error: Option<String>,
    install_mc: String,
    install_kind: LoaderKind,
    install_fabric: String,
    install_neoforge: String,
    install_forge: String,
    repair_target: Option<RepairTarget>,
    busy: Option<Task>,
    cancel_requested: bool,
    progress: Option<Progress>,
    progress_label: String,
    error: Option<String>,
    notice: Option<String>,
    log: Vec<String>,
    show_log: bool,
    saves_row: Option<Row>,
    show_saves: bool,
    saves_worlds: Vec<saves::World>,
    saves_backups: Vec<saves::Backup>,
    saves_confirm: Option<String>,
    saves_confirm_backup: Option<String>,
    saves_rx: Option<mpsc::Receiver<Msg>>,
    saves_busy: bool,
    saves_notice: Option<String>,
    saves_error: Option<String>,
    account: Option<auth::Account>,
    login_url: Option<String>,
    login_rx: Option<mpsc::Receiver<Msg>>,
    login_busy: bool,
    show_account: bool,
    rx: Option<mpsc::Receiver<Msg>>,
    catalog_rx: Option<mpsc::Receiver<Msg>>,
}

impl App {
    fn new(root: Root) -> Self {
        let java_ok = java::find_all().iter().any(|j| j.major >= 21);
        let account = auth::load(&root);
        let (icon_jobs, icon_job_rx) = mpsc::channel::<(String, String)>();
        let (icon_done, icon_rx) = mpsc::channel::<(String, Vec<u8>)>();
        std::thread::spawn(move || {
            while let Ok((key, url)) = icon_job_rx.recv() {
                if let Ok(bytes) = mods::fetch_icon(&url)
                    && icon_done.send((key, bytes)).is_err()
                {
                    break;
                }
            }
        });
        let mut app = Self {
            root,
            rows: Vec::new(),
            selected: None,
            username: "Player".into(),
            memory: 4096,
            java_ok,
            show_install: false,
            show_manage: false,
            show_mods: false,
            show_base: false,
            delete_confirm: None,
            mods_row: None,
            mods_game_dir: None,
            mods_query: String::new(),
            mods_kind: mods::ContentKind::Mod,
            mods_browse: Vec::new(),
            mods_browse_loading: false,
            mods_category: None,
            mods_source_filter: None,
            mods_mc: None,
            mods_loader: None,
            mods_chips_expanded: false,
            cf_categories: HashMap::new(),
            cf_loading: HashSet::new(),
            cf_rx: None,
            mods_shelf: None,
            mods_searching: false,
            mods_quick: None,
            mods_icons: HashMap::new(),
            mods_icons_pending: HashSet::new(),
            icon_jobs,
            icon_rx,
            mods_installed_keys: HashSet::new(),
            mods_show_installed: false,
            mods_installed_loading: false,
            mods_installed: Vec::new(),
            mods_updates_loading: false,
            mods_updates: Vec::new(),
            mods_results: Vec::new(),
            mods_total: 0,
            mods_offset: 0,
            mods_warnings: Vec::new(),
            mods_loading: false,
            mods_selected: None,
            mods_versions: Vec::new(),
            mods_version: None,
            mods_choices: Vec::new(),
            mods_optional: BTreeSet::new(),
            mods_deps_loading: false,
            mods_notice: None,
            mods_error: None,
            mods_rx: None,
            catalog: None,
            catalog_loading: false,
            catalog_error: None,
            install_mc: "1.21.1".into(),
            install_kind: LoaderKind::Fabric,
            install_fabric: "0.19.5".into(),
            install_neoforge: "21.1.209".into(),
            install_forge: "52.1.20".into(),
            repair_target: None,
            busy: None,
            cancel_requested: false,
            progress: None,
            progress_label: String::new(),
            error: None,
            notice: None,
            log: vec!["启动器已就绪".into()],
            show_log: false,
            saves_row: None,
            show_saves: false,
            saves_worlds: Vec::new(),
            saves_backups: Vec::new(),
            saves_confirm: None,
            saves_confirm_backup: None,
            saves_rx: None,
            saves_busy: false,
            saves_notice: None,
            saves_error: None,
            account,
            login_url: None,
            login_rx: None,
            login_busy: false,
            show_account: false,
            rx: None,
            catalog_rx: None,
        };
        app.refresh();
        // 调试钩子：跳过点击直接进下载中心，方便无头冒烟测试（仅 debug 构建）。
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_OPEN_MODS").is_some()
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            app.mods_row = Some(row);
            app.open_mods();
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_OPEN_SAVES").is_some()
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            app.open_saves(row);
        }
        if app.visible_count() == 0 {
            if app.rows.is_empty() {
                app.open_install();
            } else {
                app.show_manage = true;
            }
        }
        app
    }

    fn refresh(&mut self) {
        let required_by = profile_dependents(&self.root);
        let used_by = instance_usage(&self.root);
        let mut rows = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.root.versions()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().to_string();
                if !self.root.version_json(&id).exists() {
                    continue;
                }
                let (mc, loader, loader_version) = profile_info(&self.root, &id);
                let label = match &loader_version {
                    Some(version) => format!("{mc} · {} {version}", loader.name()),
                    None => mc.clone(),
                };
                let row_health = health::inspect(&self.root, &id);
                let required = required_by.get(&id).copied().unwrap_or(0);
                let used = used_by.get(&id).copied().unwrap_or(0);
                let isolated = self.root.isolated_marker(&id).exists()
                    || self.root.version_dir(&id).join("mods").is_dir();
                let game_dir = isolated.then(|| self.root.version_dir(&id));
                rows.push(Row {
                    id: id.clone(),
                    profile: id,
                    label,
                    mc,
                    loader,
                    loader_version,
                    game_dir,
                    instance: None,
                    hidden: loader == LoaderKind::Vanilla && required > 0,
                    required_by: required,
                    used_by: used,
                    health: row_health,
                });
            }
        }
        if let Ok(entries) = std::fs::read_dir(self.root.instances()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let meta_path = entry.path().join("instance.json");
                let Ok(bytes) = std::fs::read(&meta_path) else {
                    continue;
                };
                let Ok(meta) = serde_json::from_slice::<modpack::InstanceMeta>(&bytes) else {
                    continue;
                };
                if !self.root.version_json(&meta.profile).exists() {
                    continue;
                }
                let (mc, loader, loader_version) = profile_info(&self.root, &meta.profile);
                let game_dir = entry.path().join(&meta.game_dir);
                let id = format!("instance:{}", entry.file_name().to_string_lossy());
                rows.push(Row {
                    id,
                    profile: meta.profile.clone(),
                    label: format!("{} · 整合包", meta.name),
                    mc,
                    loader,
                    loader_version,
                    game_dir: Some(game_dir),
                    instance: Some(meta.name),
                    hidden: false,
                    required_by: 0,
                    used_by: 0,
                    health: health::inspect(&self.root, &meta.profile),
                });
            }
        }
        rows.sort_by(|a, b| {
            b.instance
                .is_some()
                .cmp(&a.instance.is_some())
                .then_with(|| mc_key(&b.mc).cmp(&mc_key(&a.mc)))
                .then_with(|| a.loader.rank().cmp(&b.loader.rank()))
        });
        self.rows = rows;
        self.ensure_visible_selection();
    }

    fn selected_row(&self) -> Option<Row> {
        self.selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
            .cloned()
    }

    fn is_visible(&self, row: &Row) -> bool {
        self.show_base || !row.hidden
    }

    fn visible_count(&self) -> usize {
        self.rows.iter().filter(|r| self.is_visible(r)).count()
    }

    fn visible_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| self.is_visible(r))
    }

    fn ensure_visible_selection(&mut self) {
        let valid = self
            .selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
            .is_some_and(|r| self.is_visible(r));
        if !valid {
            self.selected = self
                .rows
                .iter()
                .find(|r| self.is_visible(r))
                .map(|r| r.id.clone());
        }
    }

    fn open_install(&mut self) {
        self.show_install = true;
        self.show_manage = false;
        self.show_mods = false;
        self.error = None;
        self.notice = None;
        self.repair_target = None;
        if self.catalog.is_some() {
            self.pick_defaults();
        } else {
            self.load_catalog();
        }
    }

    fn load_catalog(&mut self) {
        if self.catalog.is_some()
            || self.catalog_loading
            || self.catalog_rx.is_some()
            || self.busy.is_some()
        {
            return;
        }
        self.catalog_loading = true;
        self.catalog_error = None;
        let (tx, rx) = mpsc::channel();
        self.catalog_rx = Some(rx);
        let root = self.root.clone();
        std::thread::spawn(move || {
            let result = catalog::load(&root, "official").map_err(|e| e.to_string());
            let _ = tx.send(Msg::Catalog(result));
        });
    }

    fn pick_defaults(&mut self) {
        let Some(cat) = self.catalog.clone() else {
            return;
        };
        let mc = if cat.minecraft.iter().any(|v| v == "1.21.1") {
            "1.21.1".to_string()
        } else {
            cat.minecraft.first().cloned().unwrap_or_default()
        };
        self.install_mc = mc.clone();
        self.install_kind = preferred_kind(&cat, &mc);
        self.install_fabric = preferred_fabric(&cat);
        self.install_neoforge = preferred_neoforge(&cat, &mc);
        self.install_forge = preferred_forge(&cat, &mc);
    }

    fn start_repair(&mut self, row: Row) {
        self.start_install_worker(row.mc, row.loader, row.loader_version.unwrap_or_default());
    }

    fn start_modpack(&mut self, path: PathBuf) {
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
        self.show_log = false;
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Install);
        self.progress = Some(Progress::stage("准备导入整合包"));
        self.progress_label = name.clone();
        self.repair_target = Some(RepairTarget::Modpack(path.clone()));
        self.log.push(format!("导入整合包 {name}"));

        let root = self.root.clone();
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
            let msg = match modpack::import_pack(&root, &path, "official", &sink) {
                Ok(instance) => Msg::InstallDone(format!("instance:{}", instance.id)),
                Err(Error::Cancelled) => Msg::InstallCancelled,
                Err(e) => Msg::InstallFailed(e.to_string()),
            };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(msg);
            }
        });
    }

    fn start_install_worker(&mut self, mc: String, loader: LoaderKind, version: String) {
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
        self.show_log = false;
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Install);
        self.progress = Some(Progress::stage("准备安装"));
        self.progress_label = target_label.clone();
        self.repair_target = Some(RepairTarget::Loader {
            mc: mc.clone(),
            loader,
            version: version.clone(),
        });
        self.install_mc = mc.clone();
        self.install_kind = loader;
        match loader {
            LoaderKind::Fabric => self.install_fabric = version.clone(),
            LoaderKind::NeoForge => self.install_neoforge = version.clone(),
            LoaderKind::Forge => self.install_forge = version.clone(),
            LoaderKind::Vanilla => {}
        }
        self.log.push(format!("安装 {target_label}"));

        let root = self.root.clone();
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
            let result: Result<String, Error> = match loader {
                LoaderKind::Vanilla => {
                    vanilla::install_with(&root, &mc, "official", &sink).map(|()| target.clone())
                }
                LoaderKind::Fabric => fabric::install_with(&root, &mc, &version, "official", &sink),
                LoaderKind::NeoForge => {
                    neoforge::install_with(&root, &mc, &version, "official", &sink)
                }
                LoaderKind::Forge => forge::install_with(&root, &mc, &version, "official", &sink),
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

    fn account_overlay(&mut self, ctx: &egui::Context) {
        if let Some(url) = &self.login_url {
            let url = url.clone();
            let mut open = false;
            let mut cancel = false;
            egui::Window::new("Microsoft 登录")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("浏览器里完成微软登录")
                                .size(16.0)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new("登录成功后会自动回到启动器")
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("重新打开浏览器").clicked() {
                                open = true;
                            }
                            if ui.button("取消").clicked() {
                                cancel = true;
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("等待授权…").color(MUTED));
                        });
                    });
                });
            if open {
                open_url(&url);
            }
            if cancel {
                cancel::cancel();
                self.login_url = None;
                self.login_rx = None;
                self.login_busy = false;
                self.notice = Some("已取消登录".into());
            }
        } else if self.show_account {
            let mut logout = false;
            let mut close = false;
            if let Some(account) = &self.account {
                let name = account.name.clone();
                let uuid = account.uuid.clone();
                egui::Window::new("账户")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(egui::RichText::new(name).size(18.0).strong());
                        ui.label(
                            egui::RichText::new(format!("UUID {uuid}"))
                                .size(10.5)
                                .color(MUTED),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("退出登录").clicked() {
                                logout = true;
                            }
                            if ui.button("关闭").clicked() {
                                close = true;
                            }
                        });
                    });
            }
            if logout {
                self.logout();
            }
            if close {
                self.show_account = false;
            }
        }
    }

    fn start_login(&mut self) {
        if self.login_busy {
            return;
        }
        cancel::reset();
        self.cancel_requested = false;
        self.login_busy = true;
        self.login_url = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.login_rx = Some(rx);
        std::thread::spawn(move || {
            let result = auth::start_auth_code().and_then(|flow| {
                let url = flow.url.clone();
                let _ = tx.send(Msg::LoginBrowser(url.clone()));
                open_url(&url);
                auth::wait_for_code(&flow, || false)
                    .and_then(|tokens| auth::login_with_msa(&tokens))
            });
            let _ = tx.send(Msg::LoginDone(result.map_err(|e| e.to_string())));
        });
    }

    fn logout(&mut self) {
        if let Err(e) = auth::clear(&self.root) {
            self.error = Some(e.to_string());
        }
        self.account = None;
        self.show_account = false;
        self.notice = Some("已退出登录".into());
    }

    fn start_launch(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.health.missing > 0 {
            self.error = Some("资源不完整，请先补全".into());
            return;
        }
        self.error = None;
        self.notice = None;
        self.busy = Some(Task::Launch);
        self.log.push(format!("启动 {}", row.id));
        let root = self.root.clone();
        let username = self.username.clone();
        let memory = self.memory;
        let account = self.account.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let mut opts = LaunchOptions {
                username,
                memory_mb: memory,
                game_dir: row.game_dir.clone(),
                ..Default::default()
            };
            if let Some(account) = account {
                match auth::ensure_fresh(&root, &account) {
                    Ok(fresh) => {
                        opts.username = fresh.name.clone();
                        opts.uuid = Some(fresh.uuid.clone());
                        opts.access_token = Some(fresh.access_token.clone());
                        opts.xuid = fresh.xuid.clone();
                        opts.client_id = Some(fresh.client_id.clone());
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::LaunchFailed(format!(
                            "账户令牌刷新失败：{e}；可在右上角退出登录改用离线账户"
                        )));
                        return;
                    }
                }
            }
            let msg = match launch::launch(&root, &row.profile, &opts) {
                Ok(0) => Msg::LaunchDone(0),
                Ok(code) => Msg::LaunchDone(code),
                Err(e) => Msg::LaunchFailed(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    fn open_mods(&mut self) {
        let row = self.selected_row();
        self.enter_mods(row);
    }

    fn enter_mods(&mut self, row: Option<Row>) {
        if self.busy.is_some() {
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
        self.mods_mc = row.as_ref().map(|row| row.mc.clone());
        self.mods_loader = row.as_ref().and_then(|row| mod_loader(row.loader));
        self.mods_row = row;
        self.mods_game_dir = game_dir;
        self.show_mods = true;
        self.show_install = false;
        self.show_manage = false;
        self.mods_kind = kind;
        self.mods_show_installed = false;
        self.mods_installed.clear();
        self.mods_installed_loading = false;
        self.mods_updates_loading = false;
        self.mods_updates.clear();
        self.mods_query.clear();
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_warnings.clear();
        self.mods_selected = None;
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_notice = None;
        self.mods_error = None;
        self.mods_loading = false;
        self.mods_deps_loading = false;
        self.mods_rx = None;
        self.mods_browse.clear();
        self.mods_browse_loading = false;
        self.mods_category = None;
        self.mods_source_filter = None;
        self.mods_chips_expanded = false;
        self.mods_shelf = None;
        self.mods_searching = false;
        self.mods_quick = None;
        self.error = None;
        self.notice = None;
        self.refresh_installed_keys();
        self.fetch_cf_categories(self.mods_kind);
        self.start_browse();
    }

    /// 只读本地索引，用来在卡片上显示“已装”。比在线扫描快，也不会占用消息通道。
    fn refresh_installed_keys(&mut self) {
        let Some(dir) = self.mods_game_dir.clone() else {
            return;
        };
        let index = installed::load(&dir);
        self.mods_installed_keys = index
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

    fn close_mods(&mut self) {
        self.show_mods = false;
        self.mods_row = None;
        self.mods_game_dir = None;
        self.mods_rx = None;
        self.mods_loading = false;
        self.mods_deps_loading = false;
        self.mods_show_installed = false;
        self.mods_installed.clear();
        self.mods_installed_loading = false;
        self.mods_updates.clear();
        self.mods_updates_loading = false;
        self.mods_notice = None;
        self.mods_error = None;
        self.mods_browse.clear();
        self.mods_browse_loading = false;
        self.mods_shelf = None;
        self.mods_searching = false;
        self.mods_quick = None;
        self.mods_source_filter = None;
        self.mods_icons.clear();
        self.mods_icons_pending.clear();
    }

    fn import_local_mod(&mut self, path: PathBuf) {
        if self.busy.is_some() {
            self.error = Some("正在安装，请等当前任务结束".into());
            return;
        }
        let Some(row) = self.selected_row() else {
            self.error = Some("先选择一个 Fabric / NeoForge / Forge 版本".into());
            return;
        };
        if mod_loader(row.loader).is_none() {
            self.error = Some("原版不能安装 Mod".into());
            return;
        }
        let game_dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        if row.instance.is_none() {
            if let Err(e) = std::fs::create_dir_all(&game_dir) {
                self.error = Some(format!("创建版本目录失败: {e}"));
                return;
            }
            if let Err(e) = std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                self.error = Some(format!("启用版本隔离失败: {e}"));
                return;
            }
        }
        match installed::import_local(&game_dir, mods::ContentKind::Mod, &path) {
            Ok(entry) => {
                self.notice = Some(format!("已导入 {}", entry.name));
                self.error = None;
                self.refresh();
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn start_mods_scan(&mut self) {
        let Some(game_dir) = self.mods_game_dir.clone() else {
            return;
        };
        self.mods_installed_loading = true;
        self.mods_error = None;
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
        std::thread::spawn(move || {
            let result = installed::scan_all(&game_dir)
                .map(|index| index.entries)
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsInstalled(result));
        });
    }

    fn start_mods_update_check(&mut self) {
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let kind = self.mods_kind;
        let loader = mod_loader(row.loader).unwrap_or(mods::Loader::Fabric);
        let entries: Vec<_> = self
            .mods_installed
            .iter()
            .filter(|entry| {
                entry.kind == kind && entry.source.is_some() && entry.project_id.is_some()
            })
            .cloned()
            .collect();
        if entries.is_empty() {
            self.mods_notice = Some("没有可检查更新的在线来源内容".into());
            return;
        }
        self.mods_updates_loading = true;
        self.mods_error = None;
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
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

    fn start_mods_update(&mut self, candidates: Vec<installed::UpdateCandidate>) {
        if self.busy.is_some() || candidates.is_empty() {
            return;
        }
        let Some(game_dir) = self.mods_game_dir.clone() else {
            return;
        };
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let kind = self.mods_kind;
        let loader = mod_loader(row.loader).unwrap_or(mods::Loader::Fabric);
        let mc = row.mc.clone();
        let dir = game_dir.join(kind.folder());
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Mods);
        self.progress = Some(Progress::stage("更新内容"));
        self.progress_label = format!("{} 个更新", candidates.len());
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

    fn start_mods_search(&mut self) {
        self.start_mods_search_at(0);
    }

    fn start_mods_search_more(&mut self) {
        let offset = self.mods_results.len();
        self.start_mods_search_at(offset);
    }

    fn start_mods_search_at(&mut self, offset: usize) {
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let _ = &row;
        let loader = self.mods_loader;
        let mc = self.mods_mc.clone();
        let query = self.mods_query.trim().to_string();
        let kind = self.mods_kind;
        self.mods_loading = true;
        self.mods_searching = true;
        self.mods_shelf = None;
        self.mods_browse.clear();
        self.mods_error = None;
        self.mods_notice = None;
        if offset == 0 {
            self.mods_warnings.clear();
            self.mods_results.clear();
            self.mods_total = 0;
            self.mods_offset = 0;
            self.mods_selected = None;
            self.mods_versions.clear();
            self.mods_version = None;
            self.mods_choices.clear();
            self.mods_optional.clear();
        }
        let source = self.mods_source_filter;
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
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
    fn show_browse(&mut self) {
        self.mods_searching = false;
        self.mods_shelf = None;
        self.mods_selected = None;
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_offset = 0;
        self.mods_query.clear();
        self.mods_error = None;
        if self.mods_browse.is_empty() && !self.mods_browse_loading {
            self.start_browse();
        }
    }

    fn reset_browse_selection(&mut self) {
        self.mods_selected = None;
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_offset = 0;
    }

    fn switch_mods_kind(&mut self, kind: mods::ContentKind) {
        if self.mods_kind == kind {
            return;
        }
        self.mods_kind = kind;
        self.mods_category = None;
        self.mods_source_filter = None;
        self.mods_shelf = None;
        self.mods_searching = false;
        self.mods_selected = None;
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_offset = 0;
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_notice = None;
        self.mods_error = None;
        self.fetch_cf_categories(kind);
        self.start_browse();
    }

    /// 按当前品类拉两排内容：热门 + 最新。
    fn start_browse(&mut self) {
        let kind = self.mods_kind;
        let loader = self.mods_loader;
        if self.mods_category.is_some()
            && self.mods_source_filter == Some(mods::Source::CurseForge)
            && !mods::curseforge_available()
        {
            self.mods_browse.clear();
            self.mods_browse_loading = false;
            self.mods_error = Some("此构建没有 CurseForge key".into());
            return;
        }
        let category = self.mods_category;
        let source = self.mods_source_filter;
        let mc = self.mods_mc.clone();
        self.mods_browse_loading = true;
        self.mods_browse.clear();
        self.mods_error = None;
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
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
                    limit: 9,
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
    fn start_shelf(&mut self, sort: mods::Sort) {
        self.mods_shelf = Some(sort);
        self.mods_searching = true;
        self.mods_selected = None;
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_offset = 0;
        self.mods_error = None;
        self.start_shelf_more(0);
    }

    fn start_shelf_more(&mut self, offset: usize) {
        let Some(sort) = self.mods_shelf else {
            return;
        };
        let kind = self.mods_kind;
        let loader = self.mods_loader;
        let category = self.mods_category;
        let source = self.mods_source_filter;
        let mc = self.mods_mc.clone();
        self.mods_loading = true;
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
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
    fn fetch_cf_categories(&mut self, kind: mods::ContentKind) {
        if self.cf_categories.contains_key(&kind) || self.cf_loading.contains(&kind) {
            return;
        }
        if !mods::curseforge_available() {
            return;
        }
        self.cf_loading.insert(kind);
        let (tx, rx) = mpsc::channel();
        self.cf_rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::curseforge_categories(kind).map_err(|e| e.to_string());
            let _ = tx.send((kind, result));
        });
    }

    /// 胶囊列表：CurseForge 品类优先，缺的再补 Modrinth 自己的品类。
    fn browse_chips(&self) -> Vec<(String, Option<mods::CategoryFilter>)> {
        let kind = self.mods_kind;
        let mut out: Vec<(String, Option<mods::CategoryFilter>)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        if let Some(items) = self.cf_categories.get(&kind)
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
    fn request_icons(&mut self, projects: impl IntoIterator<Item = mods::Project>) {
        for project in projects {
            let Some(url) = project.icon_url.clone() else {
                continue;
            };
            let key = project_key(&project);
            if self.mods_icons.contains_key(&key) || self.mods_icons_pending.contains(&key) {
                continue;
            }
            self.mods_icons_pending.insert(key.clone());
            let _ = self.icon_jobs.send((key, url));
        }
    }

    /// 把后台取回的图标解码成纹理（必须在 UI 线程做）。
    fn drain_icons(&mut self, ctx: &egui::Context) {
        let mut got = Vec::new();
        while let Ok(item) = self.icon_rx.try_recv() {
            got.push(item);
        }
        for (key, bytes) in got {
            self.mods_icons_pending.remove(&key);
            let Ok(image) = image::load_from_memory(&bytes) else {
                continue;
            };
            let rgba = image.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            let texture = ctx.load_texture(&key, color, egui::TextureOptions::LINEAR);
            self.mods_icons.insert(key, texture);
        }
    }

    /// 一键安装：不选版本、不点详情，直接装最新兼容版 + 必需前置。
    fn quick_install(&mut self, project: mods::Project) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.mods_row.clone() else {
            self.mods_error = Some("先回主界面安装一个版本，再回来点 +".into());
            return;
        };
        let kind = self.mods_kind;
        let loader = match (kind, mod_loader(row.loader)) {
            (mods::ContentKind::Mod, None) => {
                self.mods_error = Some("原版不能装 Mod，先装 Fabric / NeoForge / Forge".into());
                return;
            }
            (_, Some(loader)) => loader,
            (_, None) => mods::Loader::Fabric,
        };
        let Some(game_dir) = self.mods_game_dir.clone() else {
            self.mods_error = Some("目标目录无效".into());
            return;
        };
        if row.instance.is_none() {
            if let Some(parent) = game_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                self.mods_error = Some(format!("启用版本隔离失败: {e}"));
                return;
            }
        }
        cancel::reset();
        self.cancel_requested = false;
        self.mods_quick = Some(project_key(&project));
        self.busy = Some(Task::Mods);
        self.mods_error = None;
        self.mods_notice = None;
        self.progress = Some(Progress::stage(format!("安装 {}", project.name)));
        self.progress_label = project.name.clone();
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

    fn select_mods_project(&mut self, project: mods::Project) {
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let _ = &row;
        self.mods_loading = false;
        self.mods_browse_loading = false;
        self.mods_selected = Some(project.clone());
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_deps_loading = false;
        self.mods_error = None;
        self.mods_notice = None;
        let project_key = format!("{:?}:{}", project.source, project.id);
        let Some(mc) = self.mods_mc.clone() else {
            self.mods_error = Some("选一个具体游戏版本后可以看文件列表".into());
            return;
        };
        let loader = self.mods_loader.unwrap_or(mods::Loader::Fabric);
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::versions(&project, &mc, loader).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsVersions {
                project: project_key,
                result,
            });
        });
    }

    fn select_mods_version(&mut self, version: mods::Version) {
        let Some(mc) = self.mods_mc.clone() else {
            return;
        };
        let loader = self.mods_loader.unwrap_or(mods::Loader::Fabric);
        self.mods_version = Some(version.clone());
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_deps_loading = true;
        self.mods_error = None;
        self.mods_notice = None;
        let version_key = format!("{:?}:{}", version.source, version.id);
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
        std::thread::spawn(move || {
            let result = mods::dependency_choices(&version, &mc, loader).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsDeps {
                version: version_key,
                result,
            });
        });
    }

    fn start_mods_download(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let loader = mod_loader(row.loader).unwrap_or(mods::Loader::Fabric);
        if self.mods_kind == mods::ContentKind::Mod && mod_loader(row.loader).is_none() {
            self.mods_error = Some("原版不能安装 Mod，请切到资源包或光影".into());
            return;
        }
        let Some(version) = self.mods_version.clone() else {
            self.mods_error = Some("先选择一个 Mod 版本".into());
            return;
        };
        if self
            .mods_choices
            .iter()
            .any(|choice| choice.dependency.kind == mods::DependencyKind::Incompatible)
        {
            self.mods_error = Some("存在不兼容前置，不能下载".into());
            return;
        }
        let missing_optional = self.mods_choices.iter().any(|choice| {
            choice.dependency.kind == mods::DependencyKind::Optional
                && self.mods_optional.contains(&(
                    choice.dependency.source,
                    choice.dependency.project_id.clone(),
                ))
                && choice.version.is_none()
        });
        if missing_optional {
            self.mods_error = Some("已选的可选前置没有兼容版本".into());
            return;
        }
        let Some(game_dir) = self.mods_game_dir.clone() else {
            self.mods_error = Some("目标目录无效".into());
            return;
        };
        let kind = self.mods_kind;
        let mods_dir = game_dir.join(kind.folder());
        if let Err(e) = std::fs::create_dir_all(&mods_dir) {
            self.mods_error = Some(format!("创建 mods 目录失败: {e}"));
            return;
        }
        if row.instance.is_none() {
            if let Some(parent) = game_dir.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                self.mods_error = Some(format!("启用版本隔离失败: {e}"));
                return;
            }
        }
        cancel::reset();
        self.cancel_requested = false;
        self.busy = Some(Task::Mods);
        self.mods_error = None;
        self.mods_notice = None;
        self.progress = Some(Progress::stage(format!(
            "解析{}依赖",
            self.mods_kind.label()
        )));
        self.progress_label = version.file_name.clone();
        let optional = self.mods_optional.clone();
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

    fn poll(&mut self) {
        let mut got_cf = Vec::new();
        let mut cf_done = false;
        if let Some(rx) = &self.cf_rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => got_cf.push(item),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        cf_done = true;
                        break;
                    }
                }
            }
        }
        if cf_done {
            self.cf_rx = None;
        }
        for (kind, result) in got_cf {
            self.cf_loading.remove(&kind);
            match result {
                Ok(items) => {
                    self.cf_categories.insert(kind, items);
                }
                Err(_) => {
                    self.cf_categories.insert(kind, Vec::new());
                }
            }
        }
        let mut messages = Vec::new();
        if let Some(rx) = &self.saves_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.login_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.catalog_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.mods_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        for msg in messages {
            match msg {
                Msg::Progress(p) => {
                    if matches!(self.busy, Some(Task::Install) | Some(Task::Mods)) {
                        if self.progress.as_ref().map(|x| &x.stage) != Some(&p.stage) {
                            self.log.push(p.stage.clone());
                        }
                        self.progress = Some(p);
                    }
                }
                Msg::Catalog(result) => {
                    self.catalog_loading = false;
                    self.catalog_rx = None;
                    match result {
                        Ok(cat) => self.catalog = Some(cat),
                        Err(e) => {
                            self.catalog = Some(catalog::Catalog::fallback());
                            self.catalog_error = Some("在线版本不可用，使用内置版本".into());
                            self.log.push(format!("版本列表获取失败: {e}"));
                        }
                    }
                    if self.busy.is_none() {
                        self.pick_defaults();
                    }
                }
                Msg::InstallDone(id) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.show_install = false;
                    self.error = None;
                    self.repair_target = None;
                    self.notice = Some("安装完成".into());
                    self.refresh();
                    if self.rows.iter().any(|r| r.id == id) {
                        self.selected = Some(id);
                    }
                }
                Msg::InstallFailed(e) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.show_install = true;
                    self.show_log = true;
                    self.log.push(format!("安装未完成: {e}"));
                    if e.contains("操作已取消") {
                        self.error = None;
                        self.notice = Some("安装已取消，已下载的部分会保留。".into());
                    } else {
                        self.error = Some(e);
                    }
                    self.refresh();
                }
                Msg::InstallCancelled => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.show_install = true;
                    self.show_log = false;
                    self.error = None;
                    self.notice =
                        Some("安装已取消，已下载的部分会保留；重新开始会检查并补全。".into());
                    self.log.push("安装已取消".into());
                    self.refresh();
                }
                Msg::ModsSearch {
                    query,
                    offset,
                    result,
                } => {
                    self.mods_loading = false;
                    self.mods_rx = None;
                    if query != self.mods_query.trim() {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            if offset == 0 {
                                self.mods_results = page.hits;
                            } else {
                                for project in page.hits {
                                    if !self.mods_results.iter().any(|existing| {
                                        existing.source == project.source
                                            && existing.id == project.id
                                    }) {
                                        self.mods_results.push(project);
                                    }
                                }
                            }
                            self.mods_offset = self.mods_results.len();
                            self.mods_total = page.total.max(self.mods_offset as u64);
                            self.mods_warnings = page.warnings;
                            self.mods_error = None;
                            self.mods_searching = true;
                            let projects = self.mods_results.clone();
                            self.request_icons(projects);
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::ModsVersions { project, result } => {
                    self.mods_rx = None;
                    let Some(selected) = &self.mods_selected else {
                        continue;
                    };
                    if project != format!("{:?}:{}", selected.source, selected.id) {
                        continue;
                    }
                    match result {
                        Ok(mut versions) => {
                            self.mods_versions = std::mem::take(&mut versions);
                            self.mods_error = None;
                            if let Some(first) = self.mods_versions.first().cloned() {
                                self.select_mods_version(first);
                            }
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::ModsDeps { version, result } => {
                    self.mods_deps_loading = false;
                    self.mods_rx = None;
                    let Some(selected) = &self.mods_version else {
                        continue;
                    };
                    if version != format!("{:?}:{}", selected.source, selected.id) {
                        continue;
                    }
                    match result {
                        Ok(choices) => {
                            self.mods_choices = choices;
                            self.mods_error = None;
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::ModsDone(note) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.mods_quick = None;
                    self.mods_notice = Some(note);
                    self.mods_error = None;
                    self.refresh();
                    if self.show_mods {
                        self.start_mods_scan();
                    }
                }
                Msg::ModsFailed(e) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.mods_quick = None;
                    self.mods_error = Some(e);
                }
                Msg::ModsCancelled => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.mods_quick = None;
                    self.mods_notice = Some("Mod 下载已取消".into());
                    self.mods_error = None;
                }
                Msg::ModsInstalled(inst) => {
                    self.mods_installed_loading = false;
                    self.mods_rx = None;
                    match inst {
                        Ok(entries) => {
                            self.mods_installed_keys = entries
                                .iter()
                                .filter_map(|entry| {
                                    Some(format!(
                                        "{:?}:{}",
                                        entry.source?,
                                        entry.project_id.as_ref()?
                                    ))
                                })
                                .collect();
                            self.mods_installed = entries;
                            self.mods_updates.clear();
                            self.mods_error = None;
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::ModsUpdates(updates) => {
                    self.mods_updates_loading = false;
                    self.mods_rx = None;
                    match updates {
                        Ok(updates) => {
                            self.mods_updates = updates;
                            self.mods_error = None;
                            self.mods_notice =
                                Some(format!("检查完成，{} 个可更新", self.mods_updates.len()));
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::BrowseMore {
                    sort,
                    offset,
                    result,
                } => {
                    self.mods_loading = false;
                    self.mods_rx = None;
                    if self.mods_shelf != Some(sort) {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            if offset == 0 {
                                self.mods_results = page.hits;
                            } else {
                                for project in page.hits {
                                    if !self.mods_results.iter().any(|existing| {
                                        existing.source == project.source
                                            && existing.id == project.id
                                    }) {
                                        self.mods_results.push(project);
                                    }
                                }
                            }
                            self.mods_offset = self.mods_results.len();
                            self.mods_total = page.total.max(self.mods_offset as u64);
                            self.mods_warnings = page.warnings;
                            self.mods_error = None;
                            let projects = self.mods_results.clone();
                            self.request_icons(projects);
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::Browse {
                    kind,
                    category,
                    source,
                    result,
                } => {
                    if kind != self.mods_kind
                        || category != self.mods_category
                        || source != self.mods_source_filter
                    {
                        continue;
                    }
                    self.mods_browse_loading = false;
                    self.mods_rx = None;
                    match result {
                        Ok(shelves) => {
                            self.request_icons(
                                shelves.iter().flat_map(|shelf| shelf.projects.clone()),
                            );
                            self.mods_browse = shelves;
                            self.mods_error = None;
                        }
                        Err(e) => self.mods_error = Some(e),
                    }
                }
                Msg::SavesDone(result) => {
                    self.saves_busy = false;
                    self.saves_rx = None;
                    match result {
                        Ok(note) => {
                            self.saves_notice = Some(note);
                            self.saves_error = None;
                            self.reload_saves();
                        }
                        Err(e) => self.saves_error = Some(e),
                    }
                }
                Msg::LoginBrowser(url) => {
                    self.login_url = Some(url);
                }
                Msg::LoginDone(result) => {
                    self.login_busy = false;
                    self.login_rx = None;
                    match result {
                        Ok(account) => {
                            if let Err(e) = auth::save(&self.root, &account) {
                                self.error = Some(e.to_string());
                            }
                            self.username = account.name.clone();
                            self.account = Some(account);
                            self.login_url = None;
                            self.notice = Some("已登录 Microsoft 账户".into());
                        }
                        Err(e) => {
                            self.login_url = None;
                            self.error = Some(e);
                        }
                    }
                }
                Msg::LaunchDone(code) => {
                    self.busy = None;
                    self.rx = None;
                    if code == 0 {
                        self.notice = Some("游戏已退出".into());
                    } else {
                        self.error = Some(format!("游戏异常退出（{code}）"));
                    }
                }
                Msg::LaunchFailed(e) => {
                    self.busy = None;
                    self.rx = None;
                    self.show_log = true;
                    self.error = Some(format!("启动失败：{e}"));
                    self.log.push(format!("启动失败: {e}"));
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top")
            .default_size(64.0)
            .frame(egui::Frame::new().fill(PANEL))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(22.0);
                    logo_mark(ui);
                    ui.add_space(9.0);
                    ui.label(
                        egui::RichText::new("nailong")
                            .size(23.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(22.0);
                        if matches!(self.busy, Some(Task::Launch) | Some(Task::Mods)) {
                            ui.spinner();
                        } else if self.show_mods {
                            if ghost_button(ui, "返回").clicked() {
                                self.close_mods();
                            }
                        } else if self.show_manage {
                            if ghost_button(ui, "返回").clicked() {
                                self.show_manage = false;
                                self.delete_confirm = None;
                            }
                        } else if self.show_saves {
                            if ghost_button(ui, "返回").clicked() {
                                self.show_saves = false;
                                self.show_manage = true;
                            }
                        } else if self.busy.is_none() {
                            ui.add_space(4.0);
                            let label = self
                                .account
                                .as_ref()
                                .map(|account| account.name.clone())
                                .unwrap_or_else(|| "登录".to_string());
                            if ghost_button(ui, &label).clicked() {
                                if self.account.is_some() {
                                    self.show_account = true;
                                } else {
                                    self.start_login();
                                }
                            }
                            ui.add_space(4.0);
                            if ghost_button(ui, "下载中心").clicked() {
                                self.open_mods();
                            }
                            if !self.show_install {
                                if !self.rows.is_empty() {
                                    ui.add_space(4.0);
                                    if ghost_button(ui, "版本管理").clicked() {
                                        self.show_manage = true;
                                        self.delete_confirm = None;
                                        self.error = None;
                                    }
                                }
                                ui.add_space(4.0);
                                if ghost_button(ui, "+ 安装").clicked() {
                                    self.open_install();
                                }
                                ui.add_space(4.0);
                                if ghost_button(ui, "导入整合包").clicked()
                                    && let Some(path) = pick_modpack()
                                {
                                    self.start_modpack(path);
                                }
                            }
                        }
                    });
                });
            });
    }

    fn main_view(&mut self, ui: &mut egui::Ui) {
        let rows: Vec<Row> = self.visible_rows().cloned().collect();
        let Some(row) = self
            .selected_row()
            .filter(|r| self.is_visible(r))
            .or_else(|| rows.first().cloned())
        else {
            self.install_view(ui);
            return;
        };
        let width = ui.available_width().min(600.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        ui.add_space(36.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);

                ui.label(egui::RichText::new("游戏版本").size(12.0).color(MUTED));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let mut chosen = row.id.clone();
                    egui::ComboBox::from_id_salt("installed_version")
                        .selected_text(egui::RichText::new(&row.label).size(17.0).strong())
                        .width(430.0)
                        .show_ui(ui, |ui| {
                            for r in &rows {
                                ui.selectable_value(&mut chosen, r.id.clone(), r.label.clone());
                            }
                        });
                    if chosen != row.id {
                        self.selected = Some(chosen);
                    }
                });

                ui.add_space(30.0);
                if row.health.missing > 0 {
                    card().show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "资源不完整，缺少 {} 个文件",
                                    row.health.missing
                                ))
                                .size(15.0)
                                .color(WARN),
                            );
                            ui.add_space(14.0);
                            if primary_button(ui, "补全资源", true) {
                                self.start_repair(row.clone());
                            }
                        });
                    });
                } else {
                    let launching = matches!(self.busy, Some(Task::Launch));
                    if launching {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new("正在启动游戏…").size(15.0));
                        });
                    } else {
                        ui.vertical_centered(|ui| {
                            if primary_button(ui, "启动游戏", self.busy.is_none()) {
                                self.start_launch();
                            }
                        });
                    }
                }

                if let Some(note) = &self.notice {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(note).color(egui::Color32::from_rgb(120, 190, 140)),
                    );
                }
                if let Some(err) = &self.error {
                    ui.add_space(10.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(err).color(egui::Color32::from_rgb(230, 120, 110)),
                        )
                        .wrap(),
                    );
                }

                ui.add_space(36.0);
                card().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("玩家名").color(MUTED));
                        ui.add(egui::TextEdit::singleline(&mut self.username).desired_width(170.0));
                        ui.add_space(24.0);
                        ui.label(egui::RichText::new("内存").color(MUTED));
                        ui.add(
                            egui::DragValue::new(&mut self.memory)
                                .range(1024..=16384)
                                .suffix(" MB"),
                        );
                    });
                });
                if !self.java_ok {
                    java_warning(ui);
                }

                ui.add_space(18.0);
                egui::CollapsingHeader::new("日志")
                    .default_open(self.show_log)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(150.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for line in &self.log {
                                    ui.label(
                                        egui::RichText::new(line)
                                            .monospace()
                                            .size(11.0)
                                            .color(MUTED),
                                    );
                                }
                            });
                    });
            });
        });
    }

    fn delete_row(&mut self, row: &Row) {
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

    fn mods_view(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(850.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        self.mods_header(ui);
                        ui.add_space(14.0);
                        if self.mods_show_installed {
                            self.installed_view(ui);
                            return;
                        }
                        if self.mods_selected.is_some() {
                            self.detail_view(ui);
                            return;
                        }
                        if self.mods_searching {
                            self.results_view(ui);
                            return;
                        }
                        self.browse_view(ui);
                    });
                });
            });
    }

    fn mods_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("下载中心").size(24.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let go = ui
                    .add(
                        egui::Button::new(egui::RichText::new("搜索").size(12.0).color(MUTED))
                            .fill(PANEL)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(8)
                            .min_size(egui::vec2(54.0, 30.0)),
                    )
                    .clicked();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.mods_query)
                        .hint_text("搜索")
                        .desired_width(190.0),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if go || enter {
                    if self.mods_query.trim().is_empty() {
                        self.show_browse();
                    } else {
                        self.start_mods_search();
                    }
                }
            });
        });
        ui.add_space(12.0);
        let mut wanted_target: Option<Option<Row>> = None;
        ui.horizontal(|ui| {
            for candidate in [
                mods::ContentKind::Mod,
                mods::ContentKind::ResourcePack,
                mods::ContentKind::Shader,
            ] {
                let selected = self.mods_kind == candidate && !self.mods_show_installed;
                let fill = if selected { ACCENT } else { CARD };
                let text = if selected {
                    egui::Color32::BLACK
                } else {
                    MUTED
                };
                let button = egui::Button::new(
                    egui::RichText::new(candidate.label())
                        .size(13.0)
                        .color(text),
                )
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, LINE))
                .corner_radius(9)
                .min_size(egui::vec2(78.0, 32.0));
                if ui.add(button).clicked() {
                    self.mods_show_installed = false;
                    let changed = self.mods_kind != candidate;
                    self.switch_mods_kind(candidate);
                    if changed && self.mods_searching {
                        self.start_mods_search();
                    }
                }
            }
            ui.add_space(6.0);
            let label = if self.mods_installed.is_empty() {
                "已安装".to_string()
            } else {
                format!("已安装 {}", self.mods_installed.len())
            };
            if chip(ui, &label, self.mods_show_installed).clicked() && !self.mods_show_installed {
                if self.mods_game_dir.is_none() {
                    self.mods_error = Some("先在右边选一个安装目标，或去版本管理点「内容」".into());
                } else {
                    self.mods_show_installed = true;
                    self.start_mods_scan();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let rows = self.rows.clone();
                let label = self
                    .mods_row
                    .as_ref()
                    .map(|row| row.label.clone())
                    .unwrap_or_else(|| "只浏览".to_string());
                egui::ComboBox::from_id_salt("mods_target")
                    .selected_text(format!("安装到：{label}"))
                    .width(230.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.mods_row.is_none(), "只浏览（不安装）")
                            .clicked()
                        {
                            wanted_target = Some(None);
                        }
                        for row in &rows {
                            if ui
                                .selectable_label(
                                    self.mods_row
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
            });
        });
        if let Some(target) = wanted_target {
            self.set_mods_target(target);
        }
        ui.add_space(8.0);
        if let Some(row) = self.mods_row.clone() {
            let text = format!(
                "锁定 {} · {}，浏览、搜索和下载都只针对这个版本",
                row.mc,
                row.loader.name()
            );
            ui.label(egui::RichText::new(text).size(10.5).color(MUTED));
        } else {
            let mut wanted_mc: Option<Option<String>> = None;
            let mut wanted_loader: Option<Option<mods::Loader>> = None;
            ui.horizontal(|ui| {
                let mc_label = self
                    .mods_mc
                    .clone()
                    .unwrap_or_else(|| "全部版本".to_string());
                egui::ComboBox::from_id_salt("browse_mc")
                    .selected_text(mc_label)
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.mods_mc.is_none(), "全部版本")
                            .clicked()
                        {
                            wanted_mc = Some(None);
                        }
                        let versions = self
                            .catalog
                            .as_ref()
                            .map(|catalog| catalog.minecraft.clone())
                            .unwrap_or_else(|| vec!["1.21.1".to_string()]);
                        for version in versions.iter().take(80) {
                            if ui
                                .selectable_label(
                                    self.mods_mc.as_deref() == Some(version.as_str()),
                                    version,
                                )
                                .clicked()
                            {
                                wanted_mc = Some(Some(version.clone()));
                            }
                        }
                    });
                if self.mods_kind == mods::ContentKind::Mod {
                    let loader_label = self
                        .mods_loader
                        .map(|loader| loader.label().to_string())
                        .unwrap_or_else(|| "全部加载器".to_string());
                    egui::ComboBox::from_id_salt("browse_loader")
                        .selected_text(loader_label)
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(self.mods_loader.is_none(), "全部加载器")
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
                                        self.mods_loader == Some(loader),
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
                    egui::RichText::new("只浏览模式：不会安装，点卡片 + 会提示先选版本")
                        .size(10.5)
                        .color(MUTED),
                );
            });
            if let Some(mc) = wanted_mc {
                self.mods_mc = mc;
                if self.mods_mc.is_none() {
                    self.mods_loader = None;
                }
                self.reset_browse_selection();
                self.start_browse();
            }
            if let Some(loader) = wanted_loader
                && self.mods_loader != loader
            {
                self.mods_loader = loader;
                self.reset_browse_selection();
                self.start_browse();
            }
        }
    }

    /// 切换下载中心的安装目标；None = 只浏览。
    fn set_mods_target(&mut self, row: Option<Row>) {
        self.mods_row = row.clone();
        self.mods_game_dir = row.as_ref().map(|row| {
            row.game_dir
                .clone()
                .unwrap_or_else(|| self.root.version_dir(&row.profile))
        });
        match &row {
            Some(row) => {
                self.mods_mc = Some(row.mc.clone());
                self.mods_loader = mod_loader(row.loader);
            }
            None => {
                self.mods_mc = None;
                self.mods_loader = None;
            }
        }
        self.mods_selected = None;
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_offset = 0;
        self.mods_searching = false;
        self.mods_error = None;
        self.mods_notice = None;
        self.refresh_installed_keys();
        self.mods_installed.clear();
        if self.mods_show_installed && self.mods_game_dir.is_some() {
            self.start_mods_scan();
        }
        self.start_browse();
    }

    fn browse_view(&mut self, ui: &mut egui::Ui) {
        if mods::curseforge_available() {
            let mut wanted_source: Option<Option<mods::Source>> = None;
            ui.horizontal_wrapped(|ui| {
                for (label, value) in [
                    ("全部来源", None),
                    ("Modrinth", Some(mods::Source::Modrinth)),
                    ("CurseForge", Some(mods::Source::CurseForge)),
                ] {
                    if chip(ui, label, self.mods_source_filter == value).clicked() {
                        wanted_source = Some(value);
                    }
                }
            });
            if let Some(source) = wanted_source
                && source != self.mods_source_filter
            {
                self.mods_source_filter = source;
                self.start_browse();
            }
            ui.add_space(6.0);
        }

        let chips = self.browse_chips();
        const COLLAPSED_CHIPS: usize = 12;
        let shown = if self.mods_chips_expanded {
            chips.len()
        } else {
            chips.len().min(COLLAPSED_CHIPS)
        };
        let mut wanted: Option<Option<mods::CategoryFilter>> = None;
        let mut toggle_chips = false;
        ui.horizontal_wrapped(|ui| {
            if chip(ui, "全部", self.mods_category.is_none()).clicked() {
                wanted = Some(None);
            }
            for (label, filter) in chips.iter().take(shown) {
                if chip(ui, label, self.mods_category == *filter).clicked() {
                    wanted = Some(*filter);
                }
            }
            if chips.len() > COLLAPSED_CHIPS {
                let label = if self.mods_chips_expanded {
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
            self.mods_chips_expanded = !self.mods_chips_expanded;
        }
        if let Some(category) = wanted
            && category != self.mods_category
        {
            self.mods_category = category;
            self.start_browse();
        }

        ui.add_space(8.0);
        if let Some(error) = &self.mods_error {
            ui.label(egui::RichText::new(error).size(12.0).color(WARN));
        }
        if self.mods_browse_loading && self.mods_browse.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
            });
            return;
        }

        let per_row = if ui.available_width() >= 820.0 { 3 } else { 2 };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(240.0, 300.0);
        let shelves = std::mem::take(&mut self.mods_browse);
        if shelves.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("这里还没有内容")
                        .size(13.0)
                        .color(MUTED),
                );
            });
        }
        let mut installs = Vec::new();
        let mut more: Option<mods::Sort> = None;
        for shelf in &shelves {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(shelf.title).size(15.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("更多 ›").size(11.5).color(MUTED),
                            )
                            .fill(CARD)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(8)
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
                        if let Some(chosen) = self.project_card(ui, project, card_width) {
                            installs.push(chosen);
                        }
                    }
                });
                ui.add_space(10.0);
            }
        }
        self.mods_browse = shelves;
        if let Some(sort) = more {
            self.start_shelf(sort);
        }
        for project in installs {
            self.quick_install(project);
        }
        if let Some(note) = &self.mods_notice {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(note).size(12.0).color(OK));
        }
    }

    fn results_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("‹ 浏览").size(12.0).color(MUTED))
                        .fill(PANEL)
                        .stroke(egui::Stroke::new(1.0, LINE))
                        .corner_radius(8)
                        .min_size(egui::vec2(64.0, 28.0)),
                )
                .clicked()
            {
                self.show_browse();
            }
            let title = match self.mods_shelf {
                Some(mods::Sort::Popular) => "热门".to_string(),
                Some(mods::Sort::Newest) => "最新".to_string(),
                _ => format!("{} 个结果", self.mods_total),
            };
            ui.label(egui::RichText::new(title).size(12.0).color(MUTED));
        });
        ui.add_space(6.0);
        for warning in &self.mods_warnings {
            ui.label(egui::RichText::new(warning).size(11.0).color(WARN));
        }
        if self.mods_loading {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
            });
        }
        let per_row = if ui.available_width() >= 820.0 { 3 } else { 2 };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(240.0, 300.0);
        let results = std::mem::take(&mut self.mods_results);
        let mut installs = Vec::new();
        for chunk in results.chunks(per_row) {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for project in chunk {
                    if let Some(chosen) = self.project_card(ui, project, card_width) {
                        installs.push(chosen);
                    }
                }
            });
        }
        self.mods_results = results;
        for project in installs {
            self.quick_install(project);
        }
        if !self.mods_loading && (self.mods_results.len() as u64) < self.mods_total {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("加载更多").size(12.0).color(MUTED))
                            .fill(CARD)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(8)
                            .min_size(egui::vec2(120.0, 30.0)),
                    )
                    .clicked()
                {
                    if self.mods_shelf.is_some() {
                        self.start_shelf_more(self.mods_results.len());
                    } else {
                        self.start_mods_search_more();
                    }
                }
            });
        }
    }

    /// 画一张项目卡片；点 + 返回要安装的项目，点别处进详情。
    fn project_card(
        &mut self,
        ui: &mut egui::Ui,
        project: &mods::Project,
        width: f32,
    ) -> Option<mods::Project> {
        let key = project_key(project);
        let installed = self.mods_installed_keys.contains(&key);
        let quick = self.mods_quick.as_deref() == Some(key.as_str());
        let busy = self.busy.is_some();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 96.0), egui::Sense::click());
        let hovered = response.hovered();
        let fill = if hovered {
            egui::Color32::from_rgb(31, 35, 43)
        } else {
            CARD
        };
        let border = if hovered { ACCENT } else { LINE };
        ui.painter().rect(
            rect,
            12,
            fill,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );

        let inner = rect.shrink2(egui::vec2(10.0, 9.0));
        let mut install = None;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 10.0;

        let icon_size = egui::vec2(44.0, 44.0);
        if let Some(texture) = self.mods_icons.get(&key) {
            child.add(
                egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), icon_size))
                    .corner_radius(9),
            );
        } else {
            let (icon_rect, _) = child.allocate_exact_size(icon_size, egui::Sense::hover());
            child
                .painter()
                .rect_filled(icon_rect, 9, project_color(project));
            let letter = project
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            child.painter().text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
            );
        }

        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if installed {
                ui.label(
                    egui::RichText::new("已装")
                        .size(11.0)
                        .color(egui::Color32::from_rgb(120, 190, 140)),
                );
            } else if quick {
                ui.spinner();
            } else if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(
                        egui::RichText::new("+")
                            .size(19.0)
                            .strong()
                            .color(egui::Color32::BLACK),
                    )
                    .fill(ACCENT)
                    .corner_radius(15)
                    .min_size(egui::vec2(30.0, 30.0)),
                )
                .clicked()
            {
                install = Some(project.clone());
            }
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(egui::RichText::new(&project.name).size(13.5).strong());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(short_text(&project.summary, 72))
                            .size(10.0)
                            .color(MUTED),
                    )
                    .wrap(),
                );
                ui.label(
                    egui::RichText::new(format!("{} 下载", compact(project.downloads)))
                        .size(10.0)
                        .color(MUTED),
                );
            });
        });

        if response.clicked() && install.is_none() {
            self.select_mods_project(project.clone());
        }
        install
    }

    fn detail_view(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.mods_selected.clone() else {
            return;
        };
        if ui
            .add(
                egui::Button::new(egui::RichText::new("‹ 浏览").size(12.0).color(MUTED))
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(8)
                    .min_size(egui::vec2(64.0, 28.0)),
            )
            .clicked()
        {
            self.mods_selected = None;
            self.mods_versions.clear();
            self.mods_version = None;
            self.mods_choices.clear();
            self.mods_optional.clear();
            return;
        }
        ui.add_space(8.0);
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&project.name).size(20.0).strong());
                ui.label(
                    egui::RichText::new(project.source.label())
                        .size(11.0)
                        .color(ACCENT),
                );
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&project.summary)
                        .size(12.0)
                        .color(MUTED),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} · {} 次下载",
                    project.author,
                    compact(project.downloads)
                ))
                .size(10.5)
                .color(MUTED),
            );
            ui.add_space(10.0);

            if self.mods_versions.is_empty() {
                ui.label(egui::RichText::new("没有匹配当前版本的版本").color(WARN));
            } else {
                let versions = self.mods_versions.clone();
                let selected = self.mods_version.clone();
                let selected_text = selected
                    .as_ref()
                    .map(|v| v.version_number.clone())
                    .unwrap_or_else(|| "选择版本".into());
                let mut chosen: Option<mods::Version> = None;
                egui::ComboBox::from_id_salt("mods_version")
                    .selected_text(selected_text)
                    .width(420.0)
                    .show_ui(ui, |ui| {
                        for version in &versions {
                            if ui
                                .selectable_label(
                                    selected.as_ref().is_some_and(|v| v.id == version.id),
                                    &version.version_number,
                                )
                                .clicked()
                            {
                                chosen = Some(version.clone());
                            }
                        }
                    });
                if let Some(version) = chosen {
                    self.select_mods_version(version);
                }
            }

            if self.mods_deps_loading {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("解析前置…").color(MUTED));
                });
            } else if !self.mods_choices.is_empty() {
                ui.add_space(10.0);
                let choices = self.mods_choices.clone();
                for choice in &choices {
                    ui.horizontal(|ui| {
                        let (label, color) = match choice.dependency.kind {
                            mods::DependencyKind::Required => ("必需", ACCENT),
                            mods::DependencyKind::Optional => ("可选", MUTED),
                            mods::DependencyKind::Incompatible => ("不兼容", WARN),
                            _ => ("嵌入", MUTED),
                        };
                        ui.label(egui::RichText::new(label).size(11.0).color(color));
                        let name = choice
                            .project
                            .as_ref()
                            .map(|p| p.name.clone())
                            .or_else(|| choice.dependency.name.clone())
                            .unwrap_or_else(|| choice.dependency.project_id.clone());
                        if choice.dependency.kind == mods::DependencyKind::Optional {
                            let key = (
                                choice.dependency.source,
                                choice.dependency.project_id.clone(),
                            );
                            let mut checked = self.mods_optional.contains(&key);
                            if ui.checkbox(&mut checked, name).changed() {
                                if checked {
                                    self.mods_optional.insert(key);
                                } else {
                                    self.mods_optional.remove(&key);
                                }
                            }
                        } else {
                            ui.label(name);
                        }
                        if let Some(version) = &choice.version {
                            ui.label(
                                egui::RichText::new(&version.version_number)
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        } else {
                            ui.label(egui::RichText::new("缺失").size(11.0).color(WARN));
                        }
                    });
                }
            }

            ui.add_space(14.0);
            let incompatible = self
                .mods_choices
                .iter()
                .any(|c| c.dependency.kind == mods::DependencyKind::Incompatible);
            let missing_required = self.mods_choices.iter().any(|c| {
                c.dependency.kind == mods::DependencyKind::Required && c.version.is_none()
            });
            let can_download = self.mods_version.is_some()
                && !self.mods_deps_loading
                && !incompatible
                && !missing_required
                && self.busy.is_none();
            if let Some(note) = &self.mods_notice {
                ui.label(
                    egui::RichText::new(note)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(120, 190, 140)),
                );
                ui.add_space(4.0);
            }
            if let Some(err) = &self.mods_error {
                ui.label(
                    egui::RichText::new(err)
                        .size(12.0)
                        .color(egui::Color32::from_rgb(230, 120, 110)),
                );
                ui.add_space(4.0);
            }
            ui.vertical_centered(|ui| {
                if primary_button(ui, "安装", can_download) {
                    self.start_mods_download();
                }
            });
        });
    }

    fn saves_game_dir(&self) -> Option<PathBuf> {
        let row = self.saves_row.as_ref()?;
        Some(
            row.game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf()),
        )
    }

    fn open_saves(&mut self, row: Row) {
        if self.busy.is_some() {
            return;
        }
        self.saves_row = Some(row);
        self.show_saves = true;
        self.show_manage = false;
        self.show_mods = false;
        self.saves_confirm = None;
        self.saves_confirm_backup = None;
        self.saves_notice = None;
        self.saves_error = None;
        self.saves_busy = false;
        self.saves_rx = None;
        self.reload_saves();
    }

    fn reload_saves(&mut self) {
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        match saves::worlds(&dir) {
            Ok(items) => self.saves_worlds = items,
            Err(e) => self.saves_error = Some(e.to_string()),
        }
        match saves::backups(&dir) {
            Ok(items) => self.saves_backups = items,
            Err(e) => self.saves_error = Some(e.to_string()),
        }
    }

    fn start_backup(&mut self, name: String) {
        if self.saves_busy {
            return;
        }
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        self.saves_busy = true;
        self.saves_error = None;
        let (tx, rx) = mpsc::channel();
        self.saves_rx = Some(rx);
        std::thread::spawn(move || {
            let result = saves::backup_world(&dir, &name)
                .map(|path| {
                    format!(
                        "已备份 {} → {}",
                        name,
                        path.file_name().unwrap_or_default().to_string_lossy()
                    )
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::SavesDone(result));
        });
    }

    fn start_restore(&mut self, name: String) {
        if self.saves_busy {
            return;
        }
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        self.saves_busy = true;
        self.saves_error = None;
        let (tx, rx) = mpsc::channel();
        self.saves_rx = Some(rx);
        std::thread::spawn(move || {
            let result = saves::restore_backup(&dir, &name)
                .map(|world| format!("已恢复存档 {world}"))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::SavesDone(result));
        });
    }

    fn saves_view(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(760.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        let Some(row) = self.saves_row.clone() else {
            return;
        };
        let game_dir = self
            .saves_game_dir()
            .unwrap_or_else(|| self.root.dir().to_path_buf());
        let shared = row.game_dir.is_none();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            if ghost_button(ui, "‹ 版本管理").clicked() {
                                self.show_saves = false;
                                self.show_manage = true;
                            }
                            ui.label(
                                egui::RichText::new(format!("存档 · {}", row.label))
                                    .size(24.0)
                                    .strong(),
                            );
                        });
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(if shared {
                                "该版本还没隔离，存档在共享目录里；下载任意内容后会自动切到版本独立目录"
                            } else {
                                "存档在版本独立目录；备份会写成 zip，放在 backups/ 下"
                            })
                            .size(11.0)
                            .color(MUTED),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(game_dir.display().to_string())
                                    .size(10.0)
                                    .color(MUTED),
                            );
                            if ui.button("打开目录").clicked() {
                                open_folder(&game_dir);
                            }
                            if ui.button("刷新").clicked() {
                                self.reload_saves();
                            }
                            if ui.button("全部备份").clicked() {
                                let names: Vec<String> = self
                                    .saves_worlds
                                    .iter()
                                    .map(|world| world.name.clone())
                                    .collect();
                                for name in names {
                                    let _ = saves::backup_world(&game_dir, &name);
                                }
                                self.reload_saves();
                                self.saves_notice = Some("已备份全部存档".into());
                            }
                        });
                        ui.add_space(10.0);
                        if self.saves_busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(egui::RichText::new("正在写入备份…").color(MUTED));
                            });
                        }
                        if let Some(note) = &self.saves_notice {
                            ui.label(egui::RichText::new(note).size(12.0).color(OK));
                        }
                        if let Some(err) = &self.saves_error {
                            ui.label(egui::RichText::new(err).size(12.0).color(WARN));
                        }
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("世界").size(15.0).strong());
                        ui.add_space(6.0);
                        if self.saves_worlds.is_empty() {
                            ui.label(
                                egui::RichText::new("还没有存档。进游戏创建世界后会出现在这里。")
                                    .size(12.0)
                                    .color(MUTED),
                            );
                        }
                        let worlds = self.saves_worlds.clone();
                        for world in &worlds {
                            card().show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(&world.name)
                                                .size(14.0)
                                                .strong(),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} · {}",
                                                human_size(world.size),
                                                human_age(world.modified)
                                            ))
                                            .size(10.5)
                                            .color(MUTED),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if self.saves_confirm.as_deref()
                                                == Some(world.name.as_str())
                                            {
                                                if ui.button("取消").clicked() {
                                                    self.saves_confirm = None;
                                                }
                                                if ui
                                                    .add(
                                                        egui::Button::new("确认删除").fill(
                                                            egui::Color32::from_rgb(120, 48, 48),
                                                        ),
                                                    )
                                                    .clicked()
                                                {
                                                    match saves::delete_world(
                                                        &game_dir,
                                                        &world.name,
                                                    ) {
                                                        Ok(()) => {
                                                            self.saves_notice = Some(format!(
                                                                "已删除存档 {}",
                                                                world.name
                                                            ));
                                                            self.saves_confirm = None;
                                                            self.reload_saves();
                                                        }
                                                        Err(e) => {
                                                            self.saves_error = Some(e.to_string())
                                                        }
                                                    }
                                                }
                                            } else if ui.button("删除").clicked() {
                                                self.saves_confirm = Some(world.name.clone());
                                            }
                                            if ui.button("备份").clicked() {
                                                self.start_backup(world.name.clone());
                                            }
                                        },
                                    );
                                });
                            });
                            ui.add_space(8.0);
                        }
                        ui.add_space(14.0);
                        ui.label(egui::RichText::new("备份").size(15.0).strong());
                        ui.add_space(6.0);
                        if self.saves_backups.is_empty() {
                            ui.label(
                                egui::RichText::new("还没有备份。点上面的「备份」生成 zip。")
                                    .size(12.0)
                                    .color(MUTED),
                            );
                        }
                        let backups = self.saves_backups.clone();
                        for backup in &backups {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&backup.name).size(12.5).strong(),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {}",
                                            human_size(backup.size),
                                            human_age(backup.modified)
                                        ))
                                        .size(10.0)
                                        .color(MUTED),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if self.saves_confirm_backup.as_deref()
                                            == Some(backup.name.as_str())
                                        {
                                            if ui.button("取消").clicked() {
                                                self.saves_confirm_backup = None;
                                            }
                                            if ui.button("确认删除").clicked() {
                                                match saves::delete_backup(
                                                    &game_dir,
                                                    &backup.name,
                                                ) {
                                                    Ok(()) => {
                                                        self.saves_confirm_backup = None;
                                                        self.reload_saves();
                                                    }
                                                    Err(e) => {
                                                        self.saves_error = Some(e.to_string())
                                                    }
                                                }
                                            }
                                        } else if ui.button("删除").clicked() {
                                            self.saves_confirm_backup =
                                                Some(backup.name.clone());
                                        }
                                        if ui.button("恢复").clicked() {
                                            self.start_restore(backup.name.clone());
                                        }
                                    },
                                );
                            });
                            ui.add_space(6.0);
                            ui.separator();
                            ui.add_space(6.0);
                        }
                    });
                });
            });
    }

    fn installed_view(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        let Some(game_dir) = self.mods_game_dir.clone() else {
            return;
        };
        let kind = self.mods_kind;
        ui.horizontal(|ui| {
            if ui.button("导入本地文件").clicked()
                && let Some(path) = pick_content_file(kind)
            {
                match installed::import_local(&game_dir, kind, &path) {
                    Ok(entry) => {
                        self.mods_notice = Some(format!("已导入 {}", entry.name));
                        self.mods_error = None;
                        self.start_mods_scan();
                    }
                    Err(e) => self.mods_error = Some(e.to_string()),
                }
            }
            if ui.button("重新扫描").clicked() {
                self.start_mods_scan();
            }
            if ui.button("检查更新").clicked() {
                self.start_mods_update_check();
            }
            if !self.mods_updates.is_empty()
                && ui
                    .button(format!("全部更新 ({})", self.mods_updates.len()))
                    .clicked()
            {
                let candidates = self.mods_updates.clone();
                self.start_mods_update(candidates);
            }
        });
        if self.mods_updates_loading {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("检查版本更新…").color(MUTED));
            });
        }
        if self.mods_installed_loading {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("扫描内容目录…").color(MUTED));
            });
            return;
        }
        if let Some(note) = &self.mods_notice {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(note)
                    .size(12.0)
                    .color(egui::Color32::from_rgb(120, 190, 140)),
            );
        }
        if let Some(err) = &self.mods_error {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(err)
                    .size(12.0)
                    .color(egui::Color32::from_rgb(230, 120, 110)),
            );
        }
        let entries: Vec<installed::InstalledEntry> = self
            .mods_installed
            .iter()
            .filter(|entry| entry.kind == kind)
            .cloned()
            .collect();
        if entries.is_empty() {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(format!("{}目录里还没有内容。", kind.label())).color(MUTED),
            );
            return;
        }
        let groups = installed::duplicates(&entries);
        if !groups.is_empty() {
            ui.add_space(8.0);
            for (key, items) in groups {
                ui.label(
                    egui::RichText::new(format!(
                        "重复项目 {key}: {}",
                        items
                            .iter()
                            .map(|entry| entry.file.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .size(11.5)
                    .color(WARN),
                );
            }
        }
        if !self.mods_updates.is_empty() {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("可用更新").size(14.0).strong());
            let updates = self.mods_updates.clone();
            for candidate in &updates {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} → {}",
                            candidate.entry.name, candidate.latest.version_number
                        ))
                        .size(12.0),
                    );
                    if ui.button("更新").clicked() {
                        self.start_mods_update(vec![candidate.clone()]);
                    }
                });
            }
        }
        ui.add_space(10.0);
        for entry in entries {
            card().show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut enabled = entry.enabled;
                    if ui.checkbox(&mut enabled, "启用").changed() {
                        let mut copy = entry.clone();
                        match installed::set_enabled(&game_dir, &mut copy, enabled) {
                            Ok(()) => {
                                self.mods_notice = Some(format!(
                                    "{} {}",
                                    if enabled { "已启用" } else { "已禁用" },
                                    entry.name
                                ));
                                self.mods_error = None;
                                self.start_mods_scan();
                            }
                            Err(e) => self.mods_error = Some(e.to_string()),
                        }
                    }
                    ui.label(egui::RichText::new(&entry.name).size(14.0).strong());
                    ui.label(egui::RichText::new(&entry.file).size(10.5).color(MUTED));
                    if !entry.enabled {
                        ui.label(egui::RichText::new("已禁用").size(10.5).color(WARN));
                    }
                    if ui.button("打开目录").clicked() {
                        open_folder(&game_dir.join(kind.folder()));
                    }
                    if ui.button("删除").clicked() {
                        match installed::remove(&game_dir, &entry) {
                            Ok(()) => {
                                self.mods_notice = Some(format!("已删除 {}", entry.name));
                                self.mods_error = None;
                                self.start_mods_scan();
                            }
                            Err(e) => self.mods_error = Some(e.to_string()),
                        }
                    }
                });
                let mut meta = Vec::new();
                let file_name = if entry.enabled {
                    entry.file.clone()
                } else {
                    format!("{}.disabled", entry.file)
                };
                if let Ok(metadata) =
                    std::fs::metadata(game_dir.join(kind.folder()).join(file_name))
                {
                    meta.push(human_size(metadata.len()));
                }
                if let Some(source) = entry.source {
                    meta.push(source.label().to_string());
                }
                if let Some(version) = &entry.version_number {
                    meta.push(version.clone());
                }
                if let Some(id) = &entry.mod_id {
                    meta.push(format!("modId={id}"));
                }
                if !meta.is_empty() {
                    ui.label(
                        egui::RichText::new(meta.join(" · "))
                            .size(10.5)
                            .color(MUTED),
                    );
                }
            });
        }
    }

    fn manage_view(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(600.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        let rows = self.rows.clone();
        ui.add_space(28.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.label(egui::RichText::new("版本管理").size(28.0).strong());
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "加载器依赖的原版会作为基础版本保留，默认不显示；删除会同时移除磁盘上的文件。",
                    )
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(14.0);
                let mut show_base = self.show_base;
                if ui.checkbox(&mut show_base, "显示基础版本").changed() {
                    self.show_base = show_base;
                    self.ensure_visible_selection();
                }
                ui.add_space(10.0);

                card().show(ui, |ui| {
                    if rows.is_empty() {
                        ui.label(egui::RichText::new("还没有安装版本").color(MUTED));
                        return;
                    }
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for row in &rows {
                            let is_instance = row.instance.is_some();
                            let blocked = !is_instance && (row.required_by > 0 || row.used_by > 0);
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&row.label)
                                            .size(15.0)
                                            .strong(),
                                    );
                                    let mut detail = if is_instance {
                                        format!("整合包 · 使用 {}", row.profile)
                                    } else if row.hidden {
                                        format!(
                                            "基础版本 · 被 {} 个加载器版本依赖",
                                            row.required_by
                                        )
                                    } else {
                                        format!("{} · {}", row.mc, row.loader.name())
                                    };
                                    if !is_instance && row.used_by > 0 {
                                        detail.push_str(&format!(
                                            " · {} 个整合包在用",
                                            row.used_by
                                        ));
                                    }
                                    ui.label(
                                        egui::RichText::new(detail).size(11.0).color(MUTED),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if blocked {
                                            ui.label(
                                                egui::RichText::new("被依赖，不能删")
                                                    .size(11.0)
                                                    .color(MUTED),
                                            );
                                        } else if self.delete_confirm.as_deref()
                                            == Some(row.id.as_str())
                                        {
                                            if ui.button("取消").clicked() {
                                                self.delete_confirm = None;
                                            }
                                            if ui
                                                .add(
                                                    egui::Button::new("确认删除")
                                                        .fill(egui::Color32::from_rgb(
                                                            120, 48, 48,
                                                        )),
                                                )
                                                .clicked()
                                            {
                                                self.delete_row(row);
                                            }
                                        } else if ui
                                            .add_enabled(
                                                self.busy.is_none(),
                                                egui::Button::new(if is_instance {
                                                    "删除整合包"
                                                } else {
                                                    "删除版本"
                                                }),
                                            )
                                            .clicked()
                                        {
                                            self.delete_confirm = Some(row.id.clone());
                                        }
                                        ui.add_space(6.0);
                                        if ui
                                            .add_enabled(
                                                self.busy.is_none(),
                                                egui::Button::new("存档"),
                                            )
                                            .clicked()
                                        {
                                            self.open_saves(row.clone());
                                        }
                                        ui.add_space(4.0);
                                        if ui
                                            .add_enabled(
                                                self.busy.is_none(),
                                                egui::Button::new("内容"),
                                            )
                                            .clicked()
                                        {
                                            self.enter_mods(Some(row.clone()));
                                        }
                                    },
                                );
                            });
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(8.0);
                        }
                    });
                });

                if let Some(err) = &self.error {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(err)
                            .size(12.0)
                            .color(egui::Color32::from_rgb(230, 120, 110)),
                    );
                }
                if let Some(note) = &self.notice {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(note)
                            .size(12.0)
                            .color(egui::Color32::from_rgb(120, 190, 140)),
                    );
                }
            });
        });
    }

    fn install_view(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(600.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        ui.add_space(30.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);

                if let Some(target) = self.repair_target.clone() {
                    ui.label(egui::RichText::new("安装未完成").size(28.0).strong());
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(target.label()).color(MUTED));
                    ui.add_space(20.0);
                    card().show(ui, |ui| {
                        if let Some(note) = &self.notice {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(note)
                                        .color(egui::Color32::from_rgb(120, 190, 140)),
                                )
                                .wrap(),
                            );
                            ui.add_space(16.0);
                        }
                        if let Some(err) = &self.error {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(err)
                                        .color(egui::Color32::from_rgb(230, 120, 110)),
                                )
                                .wrap(),
                            );
                            ui.add_space(16.0);
                        }
                        ui.vertical_centered(|ui| {
                            if primary_button(ui, "继续安装", self.busy.is_none()) {
                                match target.clone() {
                                    RepairTarget::Loader {
                                        mc,
                                        loader,
                                        version,
                                    } => self.start_install_worker(mc, loader, version),
                                    RepairTarget::Modpack(path) => self.start_modpack(path),
                                }
                            }
                            ui.add_space(6.0);
                            if ui.button("返回").clicked() {
                                self.show_install = false;
                                self.repair_target = None;
                                self.error = None;
                            }
                        });
                    });
                    return;
                }

                ui.label(egui::RichText::new("安装游戏").size(28.0).strong());
                ui.add_space(20.0);

                let catalog = self
                    .catalog
                    .clone()
                    .unwrap_or_else(catalog::Catalog::fallback);
                let mut mc = self.install_mc.clone();
                let mut kind = self.install_kind;
                let mut fv = self.install_fabric.clone();
                let mut nv = self.install_neoforge.clone();
                let mut gv = self.install_forge.clone();

                card().inner_margin(22).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 24.0], egui::Label::new("Minecraft"));
                        egui::ComboBox::from_id_salt("install_mc")
                            .selected_text(mc.clone())
                            .width(250.0)
                            .show_ui(ui, |ui| {
                                for v in &catalog.minecraft {
                                    ui.selectable_value(&mut mc, v.clone(), v);
                                }
                            });
                    });
                    if mc != self.install_mc {
                        self.install_mc = mc.clone();
                        fv = preferred_fabric(&catalog);
                        nv = preferred_neoforge(&catalog, &mc);
                        gv = preferred_forge(&catalog, &mc);
                        if !available(&catalog, kind, &mc) {
                            kind = preferred_kind(&catalog, &mc);
                        }
                    }

                    ui.add_space(8.0);
                    loader_row_fabric(ui, &catalog, &mc, &mut kind, &mut fv);
                    ui.add_space(6.0);
                    loader_row_neoforge(ui, &catalog, &mc, &mut kind, &mut nv);
                    ui.add_space(6.0);
                    loader_row_forge(ui, &catalog, &mc, &mut kind, &mut gv);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [110.0, 24.0],
                                egui::RadioButton::new(kind == LoaderKind::Vanilla, "原版"),
                            )
                            .clicked()
                        {
                            kind = LoaderKind::Vanilla;
                        }
                    });

                    // Reserve the status line so loading / error text does not
                    // move the install button while the user is aiming at it.
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 24.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if self.catalog_loading {
                                ui.spinner();
                                ui.label(egui::RichText::new("获取版本…").color(MUTED));
                            } else if let Some(err) = &self.catalog_error {
                                ui.label(egui::RichText::new(err).size(11.5).color(MUTED));
                            }
                        },
                    );

                    // Persist selector changes every frame. Otherwise clicking a
                    // radio/combo would be reverted on the next repaint.
                    self.install_kind = kind;
                    self.install_fabric = fv.clone();
                    self.install_neoforge = nv.clone();
                    self.install_forge = gv.clone();

                    let version = match kind {
                        LoaderKind::Vanilla => String::new(),
                        LoaderKind::Fabric => fv.clone(),
                        LoaderKind::NeoForge => nv.clone(),
                        LoaderKind::Forge => gv.clone(),
                    };
                    let can_install = self.busy.is_none()
                        && !mc.is_empty()
                        && available(&catalog, kind, &mc)
                        && (kind == LoaderKind::Vanilla || !version.is_empty());

                    ui.add_space(22.0);
                    let mut go = false;
                    ui.vertical_centered(|ui| {
                        if primary_button(ui, "安装", can_install) {
                            go = true;
                        }
                        if self.visible_count() > 0 {
                            ui.add_space(6.0);
                            if ui.button("返回").clicked() {
                                self.show_install = false;
                                self.error = None;
                            }
                        }
                    });
                    if go {
                        self.install_mc = mc.clone();
                        self.install_kind = kind;
                        self.install_fabric = fv.clone();
                        self.install_neoforge = nv.clone();
                        self.install_forge = gv.clone();
                        self.start_install_worker(mc, kind, version);
                    }
                });

                ui.add_space(14.0);
                let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
                let mut choose_pack = false;
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, if hovering { ACCENT } else { LINE }))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new("拖入整合包（.mrpack / .zip）")
                                    .size(13.0)
                                    .color(if hovering { ACCENT } else { MUTED }),
                            );
                            ui.add_space(6.0);
                            if ghost_button(ui, "选择文件").clicked() {
                                choose_pack = true;
                            }
                        });
                    });
                if choose_pack && let Some(path) = pick_modpack() {
                    self.start_modpack(path);
                }
                if !self.java_ok {
                    java_warning(ui);
                }
            });
        });
    }

    fn progress_view(&mut self, ui: &mut egui::Ui) {
        let p = self
            .progress
            .clone()
            .unwrap_or_else(|| Progress::stage("准备安装"));
        let width = ui.available_width().min(600.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        ui.add_space(46.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                let title = if matches!(self.busy, Some(Task::Mods)) {
                    "正在下载 Mod"
                } else {
                    "正在安装"
                };
                ui.label(egui::RichText::new(title).size(26.0).strong());
                ui.add_space(6.0);
                ui.label(egui::RichText::new(&self.progress_label).color(MUTED));
                ui.add_space(28.0);
                ui.horizontal(|ui| {
                    if p.indeterminate {
                        ui.spinner();
                    }
                    ui.label(egui::RichText::new(&p.stage).size(17.0));
                });
                ui.add_space(10.0);
                if p.indeterminate {
                    ui.add(
                        egui::ProgressBar::new(0.0)
                            .desired_width(440.0)
                            .desired_height(12.0)
                            .fill(ACCENT)
                            .animate(true),
                    );
                } else {
                    ui.add(
                        egui::ProgressBar::new(p.fraction())
                            .desired_width(440.0)
                            .desired_height(12.0)
                            .fill(ACCENT)
                            .show_percentage(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(progress_detail(&p))
                            .size(12.0)
                            .color(MUTED),
                    );
                }
                ui.add_space(26.0);
                ui.vertical_centered(|ui| {
                    if self.cancel_requested {
                        ui.add_enabled(false, egui::Button::new("正在取消…"));
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "等待当前下载或 processor 结束；已完成的部分会保留。",
                            )
                            .size(11.5)
                            .color(MUTED),
                        );
                    } else if ui.button("取消安装").clicked() {
                        self.cancel_requested = true;
                        cancel::cancel();
                    }
                });
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        self.drain_icons(ui.ctx());
        let dropped = ui
            .ctx()
            .input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        for file in dropped {
            let path = file.path().to_path_buf();
            let is_pack = path
                .extension()
                .map(|e| {
                    let ext = e.to_string_lossy();
                    ext.eq_ignore_ascii_case("mrpack") || ext.eq_ignore_ascii_case("zip")
                })
                .unwrap_or(false);
            let is_mod = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"));
            if is_pack {
                if self.busy.is_none() {
                    self.start_modpack(path);
                } else {
                    self.error = Some("正在安装，请等当前任务结束".into());
                }
            } else if is_mod {
                self.import_local_mod(path);
            } else {
                self.error = Some("支持整合包 .mrpack/.zip，或单个 Mod .jar".into());
            }
        }
        if self.busy.is_some()
            || self.login_busy
            || self.saves_rx.is_some()
            || self.mods_rx.is_some()
            || self.mods_browse_loading
            || !self.mods_icons_pending.is_empty()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.top_bar(ui);
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if matches!(self.busy, Some(Task::Install) | Some(Task::Mods)) {
                self.progress_view(ui);
            } else if self.show_mods {
                self.mods_view(ui);
            } else if self.show_saves {
                self.saves_view(ui);
            } else if self.show_manage {
                self.manage_view(ui);
            } else if self.show_install {
                self.install_view(ui);
            } else {
                self.main_view(ui);
            }
        });
        self.account_overlay(ui.ctx());
    }
}

fn loader_row_fabric(
    ui: &mut egui::Ui,
    cat: &catalog::Catalog,
    mc: &str,
    kind: &mut LoaderKind,
    version: &mut String,
) {
    let enabled = catalog::fabric_supported(mc) && !cat.fabric.is_empty();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if ui
                .add_sized(
                    [110.0, 24.0],
                    egui::RadioButton::new(*kind == LoaderKind::Fabric, "Fabric"),
                )
                .clicked()
            {
                *kind = LoaderKind::Fabric;
            }
        });
        if enabled {
            let mut changed = false;
            egui::ComboBox::from_id_salt("fabric_version")
                .selected_text(if version.is_empty() {
                    "—".to_string()
                } else {
                    version.clone()
                })
                .width(250.0)
                .show_ui(ui, |ui| {
                    for v in &cat.fabric {
                        if ui.selectable_value(version, v.clone(), v).clicked() {
                            changed = true;
                        }
                    }
                });
            if changed {
                *kind = LoaderKind::Fabric;
            }
        } else {
            ui.label(egui::RichText::new("不可用").color(MUTED).size(12.0));
        }
    });
}

fn loader_row_neoforge(
    ui: &mut egui::Ui,
    cat: &catalog::Catalog,
    mc: &str,
    kind: &mut LoaderKind,
    version: &mut String,
) {
    let versions = cat.neoforge_for(mc);
    let enabled = !versions.is_empty();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if ui
                .add_sized(
                    [110.0, 24.0],
                    egui::RadioButton::new(*kind == LoaderKind::NeoForge, "NeoForge"),
                )
                .clicked()
            {
                *kind = LoaderKind::NeoForge;
            }
        });
        if enabled {
            let mut changed = false;
            egui::ComboBox::from_id_salt("neoforge_version")
                .selected_text(if version.is_empty() {
                    "—".to_string()
                } else {
                    version.clone()
                })
                .width(250.0)
                .show_ui(ui, |ui| {
                    for v in versions {
                        if ui.selectable_value(version, v.clone(), v).clicked() {
                            changed = true;
                        }
                    }
                });
            if changed {
                *kind = LoaderKind::NeoForge;
            }
        } else {
            ui.label(egui::RichText::new("不可用").color(MUTED).size(12.0));
        }
    });
}

fn loader_row_forge(
    ui: &mut egui::Ui,
    cat: &catalog::Catalog,
    mc: &str,
    kind: &mut LoaderKind,
    version: &mut String,
) {
    let versions = cat.forge_for(mc);
    let enabled = !versions.is_empty();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if ui
                .add_sized(
                    [110.0, 24.0],
                    egui::RadioButton::new(*kind == LoaderKind::Forge, "Forge"),
                )
                .clicked()
            {
                *kind = LoaderKind::Forge;
            }
        });
        if enabled {
            let mut changed = false;
            egui::ComboBox::from_id_salt("forge_version")
                .selected_text(if version.is_empty() {
                    "—".to_string()
                } else {
                    version.clone()
                })
                .width(250.0)
                .show_ui(ui, |ui| {
                    for v in versions {
                        if ui.selectable_value(version, v.clone(), v).clicked() {
                            changed = true;
                        }
                    }
                });
            if changed {
                *kind = LoaderKind::Forge;
            }
        } else {
            ui.label(egui::RichText::new("不可用").color(MUTED).size(12.0));
        }
    });
}

fn available(cat: &catalog::Catalog, kind: LoaderKind, mc: &str) -> bool {
    match kind {
        LoaderKind::Vanilla => true,
        LoaderKind::Fabric => catalog::fabric_supported(mc) && !cat.fabric.is_empty(),
        LoaderKind::NeoForge => !cat.neoforge_for(mc).is_empty(),
        LoaderKind::Forge => !cat.forge_for(mc).is_empty(),
    }
}

fn preferred_kind(cat: &catalog::Catalog, mc: &str) -> LoaderKind {
    for kind in [
        LoaderKind::Fabric,
        LoaderKind::NeoForge,
        LoaderKind::Forge,
        LoaderKind::Vanilla,
    ] {
        if available(cat, kind, mc) {
            return kind;
        }
    }
    LoaderKind::Vanilla
}

fn preferred_fabric(cat: &catalog::Catalog) -> String {
    cat.fabric.first().cloned().unwrap_or_default()
}

fn preferred_neoforge(cat: &catalog::Catalog, mc: &str) -> String {
    let list = cat.neoforge_for(mc);
    if mc == "1.21.1" && list.iter().any(|v| v == "21.1.209") {
        return "21.1.209".to_string();
    }
    list.first().cloned().unwrap_or_default()
}

fn preferred_forge(cat: &catalog::Catalog, mc: &str) -> String {
    cat.forge_for(mc).first().cloned().unwrap_or_default()
}

fn profile_dependents(root: &Root) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root.versions()) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let Ok(bytes) = std::fs::read(root.version_json(&id)) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(parent) = value.get("inheritsFrom").and_then(|v| v.as_str()) {
            *out.entry(parent.to_string()).or_insert(0) += 1;
        }
    }
    out
}

fn instance_usage(root: &Root) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(root.instances()) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(bytes) = std::fs::read(entry.path().join("instance.json")) else {
            continue;
        };
        let Ok(meta) = serde_json::from_slice::<modpack::InstanceMeta>(&bytes) else {
            continue;
        };
        *out.entry(meta.profile).or_insert(0) += 1;
    }
    out
}

fn profile_info(root: &Root, profile: &str) -> (String, LoaderKind, Option<String>) {
    let value: serde_json::Value = std::fs::read(root.version_json(profile))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let inherited = value
        .get("inheritsFrom")
        .and_then(|v| v.as_str())
        .unwrap_or(profile)
        .to_string();
    if let Some(rest) = profile.strip_prefix("fabric-loader-") {
        match rest.rsplit_once('-') {
            Some((version, mc)) => (
                mc.to_string(),
                LoaderKind::Fabric,
                Some(version.to_string()),
            ),
            None => (inherited, LoaderKind::Fabric, Some(rest.to_string())),
        }
    } else if let Some(rest) = profile.strip_prefix("neoforge-") {
        (inherited, LoaderKind::NeoForge, Some(rest.to_string()))
    } else if let Some(rest) = profile.strip_prefix("forge-") {
        match rest.split_once('-') {
            Some((mc, version)) => (mc.to_string(), LoaderKind::Forge, Some(version.to_string())),
            None => (inherited, LoaderKind::Forge, Some(rest.to_string())),
        }
    } else if let Some((mc, version)) = profile.split_once("-forge-") {
        (mc.to_string(), LoaderKind::Forge, Some(version.to_string()))
    } else {
        (profile.to_string(), LoaderKind::Vanilla, None)
    }
}

fn mc_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// 品类胶囊 / 已安装切换用的小按钮。
fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let (fill, color, border) = if selected {
        (egui::Color32::from_rgb(58, 52, 34), ACCENT, ACCENT)
    } else {
        (CARD, MUTED, LINE)
    };
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(12.0).color(color))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, border))
            .corner_radius(14)
            .min_size(egui::vec2(0.0, 26.0)),
    )
}

/// 下载量用中文单位，避免卡片里塞一长串数字。
/// 卡片上只留一小段介绍，完整描述在详情页。
fn short_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn compact(count: u64) -> String {
    if count >= 100_000_000 {
        format!("{:.1} 亿", count as f64 / 100_000_000.0)
    } else if count >= 10_000 {
        format!("{:.1} 万", count as f64 / 10_000.0)
    } else {
        count.to_string()
    }
}

fn project_color(project: &mods::Project) -> egui::Color32 {
    const PALETTE: [egui::Color32; 6] = [
        egui::Color32::from_rgb(86, 108, 168),
        egui::Color32::from_rgb(150, 96, 96),
        egui::Color32::from_rgb(96, 140, 110),
        egui::Color32::from_rgb(140, 112, 160),
        egui::Color32::from_rgb(160, 134, 84),
        egui::Color32::from_rgb(80, 128, 152),
    ];
    let hash = project.id.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as usize)
    });
    PALETTE[hash % PALETTE.len()]
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, LINE))
        .corner_radius(14)
        .inner_margin(egui::Margin::same(18))
}

fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(MUTED))
            .fill(PANEL)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(8)
            .min_size(egui::vec2(72.0, 30.0)),
    )
}

fn pick_content_file(kind: mods::ContentKind) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(format!("选择{}文件", kind.label()));
    match kind {
        mods::ContentKind::Mod => dialog.add_filter("Mod", &["jar"]).pick_file(),
        mods::ContentKind::ResourcePack | mods::ContentKind::Shader => {
            dialog.add_filter("内容包", &["zip", "jar"]).pick_file()
        }
    }
}

fn pick_modpack() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("选择整合包")
        .add_filter("整合包", &["mrpack", "zip"])
        .pick_file()
}

fn logo_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(8), ACCENT);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "N",
        egui::FontId::proportional(17.0),
        egui::Color32::BLACK,
    );
}

fn open_folder(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let size = bytes as f64;
    if size >= KIB * KIB * KIB {
        format!("{:.2} GB", size / (KIB * KIB * KIB))
    } else if size >= KIB * KIB {
        format!("{:.1} MB", size / (KIB * KIB))
    } else if size >= KIB {
        format!("{:.0} KB", size / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn human_age(modified: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let diff = now.saturating_sub(modified);
    match diff {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", diff / 60),
        3600..=86399 => format!("{} 小时前", diff / 3600),
        _ => format!("{} 天前", diff / 86400),
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn java_warning(ui: &mut egui::Ui) {
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("未检测到 Java 21，安装后将无法启动")
            .size(11.5)
            .color(WARN),
    );
}

fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let button = egui::Button::new(
        egui::RichText::new(text)
            .size(17.0)
            .strong()
            .color(egui::Color32::BLACK),
    )
    .fill(ACCENT)
    .corner_radius(10)
    .min_size(egui::vec2(200.0, 46.0));
    ui.add_enabled(enabled, button).clicked()
}

fn progress_detail(p: &Progress) -> String {
    if p.bytes_total > 0 {
        format!("{} / {}", bytes(p.bytes_done), bytes(p.bytes_total))
    } else if p.files_total > 0 {
        format!("{} / {} 个文件", p.files_done, p.files_total)
    } else {
        String::new()
    }
}

fn bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.0} KB", n / KB)
    } else {
        format!("{n:.0} B")
    }
}

fn install_cjk(ctx: &egui::Context) {
    let candidates = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("cjk".to_owned());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
}

fn main() -> eframe::Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(|p| Root::new(PathBuf::from(p)))
        .unwrap_or_else(|| Root::new(Root::default_root()));
    let options = eframe::NativeOptions {
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 640.0])
            .with_min_inner_size([760.0, 600.0])
            .with_title("nailong"),
        ..Default::default()
    };
    eframe::run_native(
        "nailong",
        options,
        Box::new(move |cc| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(18, 20, 24);
            visuals.window_fill = CARD;
            visuals.faint_bg_color = egui::Color32::from_rgb(30, 34, 41);
            visuals.selection.bg_fill = ACCENT;
            visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
            cc.egui_ctx.set_visuals(visuals);
            install_cjk(&cc.egui_ctx);
            Ok(Box::new(App::new(root)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> Root {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Root::new(
            std::env::temp_dir().join(format!("nailong-ui-{tag}-{}-{nanos}", std::process::id())),
        )
    }

    #[test]
    fn detects_loader_base_dependencies() {
        let root = temp_root("deps");
        std::fs::create_dir_all(root.versions()).unwrap();
        let base = root.versions().join("1.21.1");
        let loader = root.versions().join("neoforge-21.1.209");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&loader).unwrap();
        std::fs::write(base.join("1.21.1.json"), r#"{"id":"1.21.1"}"#).unwrap();
        std::fs::write(
            loader.join("neoforge-21.1.209.json"),
            r#"{"id":"neoforge-21.1.209","inheritsFrom":"1.21.1"}"#,
        )
        .unwrap();
        let deps = profile_dependents(&root);
        assert_eq!(deps.get("1.21.1"), Some(&1));
        let _ = std::fs::remove_dir_all(root.dir());
    }

    #[test]
    fn removal_is_direct_child_only() {
        let root = temp_root("remove");
        std::fs::create_dir_all(root.versions().join("old")).unwrap();
        std::fs::write(root.versions().join("old").join("x"), b"x").unwrap();
        root.remove_version("old").unwrap();
        assert!(!root.versions().join("old").exists());
        assert!(root.remove_version("../outside").is_err());
        assert!(root.remove_version(".").is_err());
        let _ = std::fs::remove_dir_all(root.dir());
    }
}
