#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::auth;
use nailong_core::install::{catalog, fabric, forge, health, modpack, neoforge, vanilla};
use nailong_core::installed;
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::mods;
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::rollback;
use nailong_core::saves;
use nailong_core::snapshots;
use nailong_core::{Error, Root, cancel, java};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

mod motion;

const BG: egui::Color32 = egui::Color32::from_rgb(255, 249, 240);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(245, 158, 11);
const ACCENT_HI: egui::Color32 = egui::Color32::from_rgb(255, 199, 90);
const ACCENT_LO: egui::Color32 = egui::Color32::from_rgb(240, 138, 28);
const ACCENT_TEXT: egui::Color32 = egui::Color32::from_rgb(166, 94, 10);
const INK: egui::Color32 = egui::Color32::from_rgb(46, 36, 24);
const CARD: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const PANEL: egui::Color32 = egui::Color32::from_rgb(255, 252, 246);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(255, 255, 255);
const SURFACE2: egui::Color32 = egui::Color32::from_rgb(255, 242, 224);
const LINE: egui::Color32 = egui::Color32::from_rgb(240, 224, 203);
const MUTED: egui::Color32 = egui::Color32::from_rgb(139, 122, 102);
const WARN: egui::Color32 = egui::Color32::from_rgb(190, 110, 15);
const OK: egui::Color32 = egui::Color32::from_rgb(47, 143, 91);
const SUCCESS: egui::Color32 = egui::Color32::from_rgb(47, 143, 91);
const DANGER: egui::Color32 = egui::Color32::from_rgb(192, 57, 43);

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

/// 「安装到」的完整说明：游戏版本 + 加载器 + 加载器版本。
fn target_label(row: &Row) -> String {
    match row.loader_version.as_deref() {
        Some(version) => format!("{} · {} {}", row.mc, row.loader.name(), version),
        None => format!("{} · {}", row.mc, row.loader.name()),
    }
}

/// 只有内容直接写进版本目录时，版本身份/补丁 jar 才在恢复点范围内。
fn loader_state_files(root: &Root, row: &Row, game_dir: &std::path::Path) -> Vec<String> {
    if game_dir != root.version_dir(&row.profile) {
        return Vec::new();
    }
    vec![
        format!("{}.json", row.profile),
        format!("{}.jar", row.profile),
    ]
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

/// 侧边栏的四个一级入口。玩家只需要记住这四个地方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    Home,
    Versions,
    Content,
    Saves,
}

impl Nav {
    const ALL: [Nav; 4] = [Nav::Home, Nav::Versions, Nav::Content, Nav::Saves];

    fn label(self) -> &'static str {
        match self {
            Self::Home => "启动台",
            Self::Versions => "版本",
            Self::Content => "内容",
            Self::Saves => "存档",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Home => "启动、账户与最近动态",
            Self::Versions => "安装、修复与删除版本",
            Self::Content => "Mod、资源包、光影",
            Self::Saves => "世界与快照备份",
        }
    }
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
    RollbackDone(Result<String, String>),
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

/// 安装确认里列出的一个前置。
#[derive(Clone)]
struct ReviewDependency {
    name: String,
    required: bool,
    missing: bool,
}

/// 写入磁盘之前给玩家看的最后一张清单。
#[derive(Clone)]
struct InstallReview {
    name: String,
    kind: mods::ContentKind,
    target: String,
    path: String,
    version: String,
    size: Option<u64>,
    dependencies: Vec<ReviewDependency>,
    quick: bool,
}

#[derive(Clone)]
enum InstallRequest {
    Quick(mods::Project),
    Detail,
}

struct PendingInstall {
    review: InstallReview,
    request: InstallRequest,
}

/// 启动台一帧只读一次磁盘；切页面或内容变化时再重建。
#[derive(Clone)]
struct HomeCache {
    row_id: String,
    mods: usize,
    packs: usize,
    shaders: usize,
    worlds: usize,
    latest_world: Option<saves::World>,
    preview: Vec<(String, String, u64)>,
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
    java_label: Option<String>,
    show_install: bool,
    show_manage: bool,
    show_mods: bool,
    show_base: bool,
    delete_confirm: Option<String>,
    mods_row: Option<Row>,
    mods_game_dir: Option<PathBuf>,
    mods_query: String,
    mods_filter: String,
    mods_delete_confirm: Option<String>,
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
    mods_versions_loading: bool,
    mods_version: Option<mods::Version>,
    mods_choices: Vec<mods::DependencyChoice>,
    mods_optional: BTreeSet<mods::DependencyKey>,
    mods_deps_loading: bool,
    mods_notice: Option<String>,
    mods_error: Option<String>,
    mods_rx: Option<mpsc::Receiver<Msg>>,
    /// 已安装扫描与更新检查单独走一条通道，避免覆盖浏览请求。
    mods_scan_rx: Option<mpsc::Receiver<Msg>>,
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
    saves_selected: Option<String>,
    show_saves: bool,
    saves_worlds: Vec<saves::World>,
    saves_confirm: Option<String>,
    saves_confirm_backup: Option<String>,
    saves_snapshots: HashMap<String, Vec<snapshots::Snapshot>>,
    backup_cfg: snapshots::Config,
    auto_backup_running: bool,
    saves_rx: Option<mpsc::Receiver<Msg>>,
    saves_busy: bool,
    saves_notice: Option<String>,
    saves_error: Option<String>,
    account: Option<auth::Account>,
    login_url: Option<String>,
    login_rx: Option<mpsc::Receiver<Msg>>,
    login_busy: bool,
    show_account: bool,
    show_settings: bool,
    show_rollback: bool,
    rollback_points: Vec<rollback::Snapshot>,
    rollback_busy: bool,
    rollback_confirm: Option<String>,
    rollback_notice: Option<String>,
    rollback_error: Option<String>,
    rollback_rx: Option<mpsc::Receiver<Msg>>,
    pending_install: Option<PendingInstall>,
    home_cache: Option<HomeCache>,
    version_cache: Option<(String, (usize, usize, usize, usize))>,
    page_key: u8,
    page_enter_at: f64,
    modal_key: u8,
    modal_started: f64,
    rx: Option<mpsc::Receiver<Msg>>,
    catalog_rx: Option<mpsc::Receiver<Msg>>,
}

/// 玩家名与内存存在数据目录的 `ui.json`，随设置弹窗关闭落盘。
fn load_ui_prefs(root: &Root) -> (String, u32) {
    let value = std::fs::read(root.dir().join("ui.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let username = value
        .as_ref()
        .and_then(|value| value.get("username"))
        .and_then(|value| value.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Player")
        .to_string();
    let memory = value
        .as_ref()
        .and_then(|value| value.get("memory"))
        .and_then(|value| value.as_u64())
        .unwrap_or(4096)
        .clamp(1024, 16384) as u32;
    (username, memory)
}

impl App {
    fn new(root: Root) -> Self {
        let javas = java::find_all();
        let java_ok = javas.iter().any(|j| j.major >= 21);
        let java_label = javas.iter().find(|j| j.major >= 21).map(|j| j.label());
        let account = auth::load(&root);
        let backup_cfg = snapshots::load_config(&root);
        let (username, memory) = load_ui_prefs(&root);
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
            username,
            memory,
            java_ok,
            java_label,
            show_install: false,
            show_manage: false,
            show_mods: false,
            show_base: false,
            delete_confirm: None,
            mods_row: None,
            mods_game_dir: None,
            mods_query: String::new(),
            mods_filter: String::new(),
            mods_delete_confirm: None,
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
            mods_versions_loading: false,
            mods_version: None,
            mods_choices: Vec::new(),
            mods_optional: BTreeSet::new(),
            mods_deps_loading: false,
            mods_notice: None,
            mods_error: None,
            mods_rx: None,
            mods_scan_rx: None,
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
            saves_selected: None,
            show_saves: false,
            saves_worlds: Vec::new(),
            saves_confirm: None,
            saves_confirm_backup: None,
            saves_snapshots: HashMap::new(),
            backup_cfg,
            auto_backup_running: false,
            saves_rx: None,
            saves_busy: false,
            saves_notice: None,
            saves_error: None,
            account,
            login_url: None,
            login_rx: None,
            login_busy: false,
            show_account: false,
            show_settings: false,
            show_rollback: false,
            rollback_points: Vec::new(),
            rollback_busy: false,
            rollback_confirm: None,
            rollback_notice: None,
            rollback_error: None,
            rollback_rx: None,
            pending_install: None,
            home_cache: None,
            version_cache: None,
            page_key: 255,
            page_enter_at: 0.0,
            modal_key: 0,
            modal_started: 0.0,
            rx: None,
            catalog_rx: None,
        };
        app.refresh();
        // 调试钩子：跳过点击直接进下载中心，方便无头冒烟测试（仅 debug 构建）。
        #[cfg(debug_assertions)]
        if let Ok(id) = std::env::var("NAILONG_DEBUG_SELECT")
            && let Some(row) = app.rows.iter().find(|row| row.id.contains(&id))
        {
            app.selected = Some(row.id.clone());
        }
        #[cfg(debug_assertions)]
        if let Some(screen) = std::env::var_os("NAILONG_DEBUG_SCREEN") {
            let screen = screen.to_string_lossy().to_string();
            let row = app.selected_row().or_else(|| app.rows.first().cloned());
            match screen.as_str() {
                "install" => app.open_install(),
                "manage" => {
                    app.show_manage = true;
                    app.show_install = false;
                }
                "mods" => {
                    if row.is_some() {
                        app.mods_row = row.clone();
                        app.open_mods();
                        // Preload the installed index so the tab count and
                        // "已装" badges match what the player sees after
                        // visiting the installed tab once.
                        app.start_mods_scan();
                    }
                }
                "saves" => {
                    if let Some(row) = row {
                        app.open_saves(row);
                    }
                }
                "installed" => {
                    if let Some(row) = row {
                        app.mods_row = Some(row);
                        app.open_mods();
                        app.mods_show_installed = true;
                        app.start_mods_scan();
                    }
                }
                "account" => {
                    app.account = Some(auth::Account {
                        name: "Steve".into(),
                        uuid: "069a79f444e94726a5befca90e38aaf5".into(),
                        access_token: "debug".into(),
                        refresh_token: "debug".into(),
                        expires_at: u64::MAX / 2,
                        xuid: None,
                        client_id: String::new(),
                    });
                    app.show_account = true;
                }
                "login" => {
                    app.login_url = Some(
                        "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
                    );
                    app.login_busy = true;
                }
                "progress" => {
                    app.busy = Some(Task::Install);
                    app.progress_label = "NeoForge 21.1.264".into();
                    app.progress = Some(Progress {
                        stage: "下载资源文件".into(),
                        files_done: 812,
                        files_total: 3911,
                        bytes_done: 180_000_000,
                        bytes_total: 320_000_000,
                        indeterminate: false,
                    });
                }
                _ => {}
            }
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_OPEN_MODS").is_some()
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            app.mods_row = Some(row);
            app.open_mods();
        }
        #[cfg(debug_assertions)]
        if let Ok(term) = std::env::var("NAILONG_DEBUG_PROJECT")
            && app.show_mods
            && app.mods_selected.is_none()
            && let Ok(page) = mods::search(
                &term,
                None,
                app.mods_mc.as_deref(),
                app.mods_loader,
                app.mods_kind,
                0,
                8,
            )
            && let Some(project) = page.hits.into_iter().next()
        {
            app.request_icons(std::iter::once(project.clone()));
            app.select_mods_project(project);
        }
        #[cfg(debug_assertions)]
        if let Ok(term) = std::env::var("NAILONG_DEBUG_SEARCH")
            && app.show_mods
        {
            app.mods_query = term;
            app.start_mods_search();
        }
        #[cfg(debug_assertions)]
        if std::env::var("NAILONG_DEBUG_REVIEW").as_deref() == Ok("quick")
            && app.show_mods
            && let Some(project) = app.mods_selected.clone()
        {
            app.review_quick_install(project);
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_OPEN_SAVES").is_some()
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            app.open_saves(row);
        }
        if app.visible_count() == 0 && !app.rows.is_empty() {
            app.show_manage = true;
        }
        #[cfg(debug_assertions)]
        if std::env::var("NAILONG_DEBUG_SCREEN").as_deref() == Ok("welcome") {
            app.show_install = false;
            app.show_manage = false;
            app.show_mods = false;
            app.show_saves = false;
        }
        app
    }

    fn page_id(&self) -> u8 {
        if matches!(self.busy, Some(Task::Install) | Some(Task::Mods)) {
            return 10;
        }
        if self.show_mods {
            if self.mods_show_installed {
                6
            } else if self.mods_selected.is_some() {
                5
            } else if self.mods_searching {
                4
            } else {
                3
            }
        } else if self.show_saves {
            8
        } else if self.show_manage {
            2
        } else if self.show_install {
            1
        } else {
            0
        }
    }

    fn refresh(&mut self) {
        self.home_cache = None;
        self.version_cache = None;
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
            // 首屏优先选“看起来像常用版本”的那个：有整合包实例、有内容、
            // 安装完整。没有任何线索时退回列表最后一个，避免总选到空版本。
            self.selected = self
                .rows
                .iter()
                .filter(|row| self.is_visible(row))
                .max_by_key(|row| {
                    let dir = row
                        .game_dir
                        .clone()
                        .unwrap_or_else(|| self.root.version_dir(&row.profile));
                    let content = installed::load(&dir).entries.len();
                    (row.instance.is_some(), content, row.health.missing == 0)
                })
                .map(|row| row.id.clone());
        }
    }

    fn open_install(&mut self) {
        self.show_install = true;
        self.show_manage = false;
        self.show_mods = false;
        self.show_saves = false;
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
        let now = ctx.input(|input| input.time);
        let kind = if self.login_url.is_some() {
            1
        } else if self.show_account && self.account.is_some() {
            2
        } else {
            0
        };
        if kind != self.modal_key {
            self.modal_key = kind;
            self.modal_started = now;
        }
        let raw = (((now - self.modal_started) / 0.20) as f32).clamp(0.0, 1.0);
        let open_e = motion::ease_out_cubic(raw);
        if kind != 0 && raw < 1.0 {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let backdrop = (72.0 * open_e) as u8;
        let veil = egui::Color32::from_rgba_unmultiplied(46, 36, 24, backdrop);
        let frame_fill = egui::Color32::from_rgba_unmultiplied(
            CARD.r(),
            CARD.g(),
            CARD.b(),
            (255.0 * open_e) as u8,
        );
        let frame_stroke = egui::Color32::from_rgba_unmultiplied(
            LINE.r(),
            LINE.g(),
            LINE.b(),
            (255.0 * open_e) as u8,
        );
        if let Some(url) = self.login_url.clone() {
            let mut open = false;
            let mut cancel_login = false;
            let modal = egui::Modal::new(egui::Id::unique("login_modal"))
                .backdrop_color(veil)
                .frame(
                    egui::Frame::new()
                        .fill(frame_fill)
                        .stroke(egui::Stroke::new(1.0, frame_stroke))
                        .corner_radius(16)
                        .shadow(motion::card_shadow())
                        .inner_margin(egui::Margin::symmetric(26, 22)),
                )
                .show(ctx, |ui| {
                    ui.multiply_opacity(0.35 + 0.65 * open_e);
                    ui.add_space((1.0 - open_e) * 8.0);
                    ui.set_width(368.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("在浏览器中完成登录")
                                .size(19.0)
                                .family(bold_family()),
                        );
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "登录成功后会自动回到启动器，不需要手动复制代码。",
                                )
                                .size(10.5)
                                .color(MUTED),
                            )
                            .wrap(),
                        );
                        ui.add_space(14.0);
                        egui::Frame::new()
                            .fill(PANEL)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(10)
                            .inner_margin(egui::Margin::symmetric(14, 11))
                            .show(ui, |ui| {
                                ui.set_width(310.0);
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new("等待微软授权…")
                                            .size(11.0)
                                            .color(MUTED),
                                    );
                                });
                            });
                        ui.add_space(14.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(188.0, 34.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                if accent_outline(ui, "重新打开浏览器").clicked() {
                                    open = true;
                                }
                                ui.add_space(6.0);
                                if ghost_button(ui, "取消").clicked() {
                                    cancel_login = true;
                                }
                            },
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("浏览器没有自动打开？点上面的按钮再试一次")
                                .size(9.5)
                                .color(MUTED),
                        );
                    });
                });
            if modal.backdrop_response.clicked() {
                cancel_login = true;
            }
            if open {
                open_url(&url);
            }
            if cancel_login {
                // 登录和安装共用的是一个“前台任务取消”标志；登录窗口的取消
                // 只是关闭等待，不能让之后的内容浏览误判成“操作已取消”。
                cancel::reset();
                self.login_url = None;
                self.login_rx = None;
                self.login_busy = false;
                self.notice = Some("已取消登录".into());
            }
        } else if self.show_account
            && let Some(account) = &self.account
        {
            let name = account.name.clone();
            let uuid = account.uuid.clone();
            let mut logout = false;
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("account_modal"))
                .backdrop_color(veil)
                .frame(
                    egui::Frame::new()
                        .fill(frame_fill)
                        .stroke(egui::Stroke::new(1.0, frame_stroke))
                        .corner_radius(16)
                        .shadow(motion::card_shadow())
                        .inner_margin(egui::Margin::symmetric(24, 20)),
                )
                .show(ctx, |ui| {
                    ui.multiply_opacity(0.35 + 0.65 * open_e);
                    ui.add_space((1.0 - open_e) * 8.0);
                    ui.set_width(360.0);
                    ui.horizontal(|ui| {
                        avatar_sized(ui, &name, 46.0);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&name).size(20.0).family(bold_family()));
                            ui.horizontal(|ui| {
                                status_dot(ui, true);
                                ui.label(
                                    egui::RichText::new("Microsoft 账户 · 已登录")
                                        .size(10.0)
                                        .color(MUTED),
                                );
                            });
                        });
                    });
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("UUID").size(9.5).color(MUTED));
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(short_text(&uuid, 32))
                                    .size(10.0)
                                    .color(INK),
                            )
                            .truncate(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_link(ui, "复制").clicked() {
                                ui.ctx().copy_text(uuid.clone());
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("启动时自动使用这个账户，令牌过期会自动续期")
                            .size(9.5)
                            .color(MUTED),
                    );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("退出登录").size(11.0).color(DANGER),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            logout = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary_wide(ui, "关闭", true, 96.0) {
                                close = true;
                            }
                        });
                    });
                });
            if modal.backdrop_response.clicked() {
                close = true;
            }
            if logout {
                self.logout();
            }
            if close {
                self.show_account = false;
            }
        }
    }

    /// 设置不再是首页的一块卡片，也不是第五个一级页面；它只在这里出现。
    fn settings_overlay(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let game_dir = self
            .selected_row()
            .map(|row| {
                row.game_dir
                    .clone()
                    .unwrap_or_else(|| self.root.version_dir(&row.profile))
            })
            .unwrap_or_else(|| self.root.dir().to_path_buf());
        let mut close = false;
        let mut open_account = false;
        let mut open_dir: Option<PathBuf> = None;
        let mut refresh = false;
        let veil = egui::Color32::from_rgba_unmultiplied(46, 36, 24, 72);
        let modal = egui::Modal::new(egui::Id::unique("settings_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(16)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(470.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("设置").size(20.0).family(bold_family()));
                        ui.label(
                            egui::RichText::new("账户、内存、Java 与本地目录")
                                .size(10.0)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .size(9.0)
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(14.0);

                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(442.0);
                        ui.horizontal(|ui| {
                            if let Some(account) = &self.account {
                                avatar_sized(ui, &account.name, 30.0);
                                ui.add_space(8.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&account.name)
                                            .size(13.0)
                                            .family(bold_family()),
                                    );
                                    ui.label(
                                        egui::RichText::new("Microsoft 账户 · 已登录")
                                            .size(9.0)
                                            .color(OK),
                                    );
                                });
                            } else {
                                avatar_sized(ui, &self.username, 30.0);
                                ui.add_space(8.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new(&self.username)
                                            .size(13.0)
                                            .family(bold_family()),
                                    );
                                    ui.label(
                                        egui::RichText::new("离线账户 · 单人 / 局域网")
                                            .size(9.0)
                                            .color(MUTED),
                                    );
                                });
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let label = if self.account.is_some() {
                                        "管理账户"
                                    } else {
                                        "登录 Microsoft"
                                    };
                                    if motion::secondary_sized(ui, label, true, 112.0, 30.0, 10.5)
                                        .clicked()
                                    {
                                        open_account = true;
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(12.0);

                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(442.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("玩家名").size(10.5).color(MUTED));
                            text_field(ui, &mut self.username, "", 150.0);
                            ui.add_space(12.0);
                            ui.label(egui::RichText::new("内存").size(10.5).color(MUTED));
                            ui.add(
                                egui::DragValue::new(&mut self.memory)
                                    .range(1024..=16384)
                                    .suffix(" MB"),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    status_dot(ui, self.java_ok);
                                    ui.label(
                                        egui::RichText::new(if self.java_ok {
                                            "Java 21 已检测"
                                        } else {
                                            "缺少 Java 21"
                                        })
                                        .size(9.5)
                                        .color(if self.java_ok { MUTED } else { WARN }),
                                    );
                                },
                            );
                        });
                    });
                ui.add_space(12.0);

                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.set_width(442.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("游戏目录").size(10.5).color(MUTED));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if motion::secondary_sized(ui, "打开", true, 54.0, 24.0, 10.0)
                                        .clicked()
                                    {
                                        open_dir = Some(game_dir.clone());
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(game_dir.display().to_string())
                                                .size(9.0)
                                                .color(MUTED),
                                        )
                                        .truncate(),
                                    );
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("数据目录").size(10.5).color(MUTED));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if motion::secondary_sized(ui, "打开", true, 54.0, 24.0, 10.0)
                                        .clicked()
                                    {
                                        open_dir = Some(self.root.dir().to_path_buf());
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(
                                                self.root.dir().display().to_string(),
                                            )
                                            .size(9.0)
                                            .color(MUTED),
                                        )
                                        .truncate(),
                                    );
                                },
                            );
                        });
                    });
                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("诊断")
                            .size(10.5)
                            .family(bold_family())
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if action_link(ui, "刷新版本列表").clicked() {
                            refresh = true;
                        }
                    });
                });
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!("日志（最近 {} 条）", self.log.len().min(80)))
                        .size(10.0)
                        .color(MUTED),
                )
                .default_open(self.show_log)
                .show(ui, |ui| {
                    egui::Frame::new()
                        .fill(PANEL)
                        .stroke(egui::Stroke::new(1.0, LINE))
                        .corner_radius(10)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.set_width(444.0);
                            egui::ScrollArea::vertical()
                                .max_height(110.0)
                                .auto_shrink([false, false])
                                .stick_to_bottom(true)
                                .show(ui, |ui| {
                                    for line in self.log.iter().rev().take(80).rev() {
                                        ui.label(
                                            egui::RichText::new(line)
                                                .monospace()
                                                .size(9.5)
                                                .color(MUTED),
                                        );
                                    }
                                });
                        });
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if motion::secondary_sized(ui, "打开数据目录", true, 118.0, 32.0, 11.0)
                        .clicked()
                    {
                        open_dir = Some(self.root.dir().to_path_buf());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::primary_sized(ui, "完成", true, 86.0, 32.0, 12.0) {
                            close = true;
                        }
                    });
                });
            });
        if modal.backdrop_response.clicked() {
            close = true;
        }
        if open_account {
            self.save_ui_config();
            self.show_settings = false;
            if self.account.is_some() {
                self.show_account = true;
            } else {
                self.start_login();
            }
        }
        if let Some(dir) = open_dir {
            open_folder(&dir);
        }
        if refresh {
            self.refresh();
        }
        if close {
            self.save_ui_config();
            self.show_settings = false;
        }
    }

    fn save_ui_config(&self) {
        let value = serde_json::json!({
            "username": self.username,
            "memory": self.memory,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
            let _ = std::fs::write(self.root.dir().join("ui.json"), bytes);
        }
    }

    fn reload_rollback_points(&mut self) {
        let Some(dir) = self.mods_game_dir.clone() else {
            self.rollback_points.clear();
            return;
        };
        match rollback::list(&dir) {
            Ok(items) => {
                self.rollback_points = items;
                self.rollback_error = None;
            }
            Err(e) => self.rollback_error = Some(e.to_string()),
        }
    }

    fn open_rollback(&mut self) {
        if self.mods_game_dir.is_none() {
            self.mods_error = Some("先选一个安装目标，再查看恢复点".into());
            return;
        }
        self.show_rollback = true;
        self.rollback_confirm = None;
        self.rollback_notice = None;
        self.rollback_error = None;
        self.reload_rollback_points();
    }

    fn start_rollback_backup(&mut self) {
        if self.rollback_busy {
            return;
        }
        let Some(dir) = self.mods_game_dir.clone() else {
            return;
        };
        self.rollback_busy = true;
        self.rollback_notice = None;
        self.rollback_error = None;
        let (tx, rx) = mpsc::channel();
        self.rollback_rx = Some(rx);
        let loader = self
            .mods_row
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

    fn start_rollback_restore(&mut self, id: String) {
        if self.rollback_busy {
            return;
        }
        let Some(dir) = self.mods_game_dir.clone() else {
            return;
        };
        self.rollback_busy = true;
        self.rollback_notice = None;
        self.rollback_error = None;
        self.rollback_confirm = None;
        let (tx, rx) = mpsc::channel();
        self.rollback_rx = Some(rx);
        std::thread::spawn(move || {
            let result = rollback::restore(&dir, &id)
                .map(|count| format!("已恢复 {count} 个文件；恢复前的状态也留了恢复点"))
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::RollbackDone(result));
        });
    }

    /// 内容页的“恢复点”入口：恢复 Mod / 配置 / 内容索引，不动存档。
    fn rollback_overlay(&mut self, ctx: &egui::Context) {
        if !self.show_rollback {
            return;
        }
        let mut close = false;
        let mut create = false;
        let mut restore: Option<String> = None;
        let mut delete: Option<String> = None;
        let veil = egui::Color32::from_rgba_unmultiplied(46, 36, 24, 72);
        let modal = egui::Modal::new(egui::Id::unique("rollback_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(16)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("恢复点")
                                .size(20.0)
                                .family(bold_family()),
                        );
                        ui.label(
                            egui::RichText::new(
                                "恢复 Mod、配置、内容索引和加载器版本文件；恢复前会自动留一份当前状态",
                            )
                            .size(10.0)
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("保留最近 {} 个", rollback::KEEP))
                                .size(9.0)
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(12.0);
                if let Some(note) = &self.rollback_notice {
                    ui.label(egui::RichText::new(note).size(10.0).color(OK));
                    ui.add_space(6.0);
                }
                if let Some(err) = &self.rollback_error {
                    ui.label(egui::RichText::new(err).size(10.0).color(DANGER));
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if self.rollback_busy {
                        ui.spinner();
                        ui.label(egui::RichText::new("正在处理内容…").size(10.0).color(MUTED));
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
                        if self.rollback_points.is_empty() {
                            ui.add_space(18.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("还没有恢复点")
                                        .size(14.0)
                                        .family(bold_family()),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "安装或更新内容前会自动创建一个；版本目录内的加载器 JSON / 补丁 jar 也会一起保存。",
                                    )
                                    .size(10.0)
                                    .color(MUTED),
                                );
                            });
                            ui.add_space(18.0);
                        }
                        for point in &self.rollback_points {
                            egui::Frame::new()
                                .fill(PANEL)
                                .stroke(egui::Stroke::new(1.0, LINE))
                                .corner_radius(10)
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
                                                .size(11.5)
                                                .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} 个文件 · {}",
                                                    point.files.len(),
                                                    human_size(point.bytes)
                                                ))
                                                .size(9.0)
                                                .color(MUTED),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if self.rollback_confirm.as_deref()
                                                    == Some(point.id.as_str())
                                                {
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new("确认恢复")
                                                                    .size(10.0)
                                                                    .color(DANGER),
                                                            )
                                                            .fill(egui::Color32::from_rgb(
                                                                255, 226, 220,
                                                            ))
                                                            .stroke(egui::Stroke::new(1.0, DANGER)),
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
                                                        self.rollback_confirm = None;
                                                    }
                                                } else {
                                                    if motion::secondary_sized(
                                                        ui, "恢复", true, 56.0, 24.0, 10.0,
                                                    )
                                                    .clicked()
                                                    {
                                                        self.rollback_confirm =
                                                            Some(point.id.clone());
                                                    }
                                                    if ui
                                                        .add(
                                                            egui::Button::new(
                                                                egui::RichText::new("删除")
                                                                    .size(10.0)
                                                                    .color(MUTED),
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
                        .size(9.0)
                        .color(MUTED),
                );
            });
        if modal.backdrop_response.clicked() {
            close = true;
        }
        if create {
            self.start_rollback_backup();
        }
        if let Some(id) = restore {
            self.start_rollback_restore(id);
        }
        if let Some(id) = delete
            && let Some(dir) = self.mods_game_dir.clone()
        {
            match rollback::delete(&dir, &id) {
                Ok(()) => {
                    self.rollback_notice = Some("已删除恢复点".into());
                    self.reload_rollback_points();
                }
                Err(e) => self.rollback_error = Some(e.to_string()),
            }
        }
        if close {
            self.show_rollback = false;
        }
    }

    /// 安装确认：玩家点下安装之后、真正写盘之前，再看一眼清单。
    fn install_review_overlay(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_install.as_ref() else {
            return;
        };
        let review = pending.review.clone();
        let mut cancel = false;
        let mut confirm = false;
        let veil = egui::Color32::from_rgba_unmultiplied(46, 36, 24, 72);
        let modal = egui::Modal::new(egui::Id::unique("install_review_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(16)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(500.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("确认安装")
                                .size(20.0)
                                .family(bold_family()),
                        );
                        ui.label(
                            egui::RichText::new("先确认写入位置，再开始下载")
                                .size(10.0)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        tag_fill(ui, review.kind.label(), MUTED, SURFACE2);
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
                                .size(9.0)
                                .color(MUTED),
                        );
                    }
                });
                ui.add_space(9.0);
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(476.0);
                        install_review_row(ui, "安装到", &review.target);
                        ui.add_space(5.0);
                        install_review_row(ui, "写入目录", &review.path);
                        ui.add_space(5.0);
                        install_review_row(ui, "版本", &review.version);
                        if let Some(size) = review.size {
                            ui.add_space(5.0);
                            install_review_row(ui, "预计大小", &human_size(size));
                        }
                        ui.add_space(5.0);
                        install_review_row(ui, "影响范围", "只写这个版本的内容目录，不动世界存档");
                    });
                ui.add_space(12.0);
                if review.quick {
                    ui.label(
                        egui::RichText::new(
                            "点「确认安装」后才会解析最新兼容版本与必需前置；解析失败不会改动现有文件。",
                        )
                        .size(10.0)
                        .color(MUTED),
                    );
                } else if review.dependencies.is_empty() {
                    ui.horizontal(|ui| {
                        status_dot(ui, true);
                        ui.label(
                            egui::RichText::new("没有额外前置，只安装这一个文件")
                                .size(10.5)
                                .color(MUTED),
                        );
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "一起安装 {} 个前置",
                                review.dependencies.len()
                            ))
                            .size(11.5)
                            .family(bold_family()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new("必需自动勾选，可选按你的选择")
                                    .size(9.0)
                                    .color(MUTED),
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
                                            ACCENT_TEXT,
                                            egui::Color32::from_rgb(255, 243, 214),
                                        );
                                    } else {
                                        tag_fill(ui, "可选", MUTED, SURFACE2);
                                    }
                                    ui.add_space(3.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(short_text(
                                                &dependency.name,
                                                44,
                                            ))
                                            .size(11.0),
                                        )
                                        .truncate(),
                                    );
                                    if dependency.missing {
                                        ui.label(
                                            egui::RichText::new("缺失")
                                                .size(9.0)
                                                .color(WARN),
                                        );
                                    }
                                });
                                ui.add_space(4.0);
                            }
                        });
                }
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(255, 248, 232))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(244, 222, 178)))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(476.0);
                        ui.label(
                            egui::RichText::new("安装前自动创建恢复点")
                                .size(11.0)
                                .family(bold_family())
                                .color(ACCENT_TEXT),
                        );
                        ui.label(
                            egui::RichText::new(
                                "装完发现不对，在「内容 › 恢复点」一键回到现在。",
                            )
                            .size(9.5)
                            .color(MUTED),
                        );
                    });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::primary_sized_glyph(
                            ui,
                            "确认安装",
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

    #[cfg(debug_assertions)]
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
        self.mods_filter.clear();
        self.mods_delete_confirm = None;
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_warnings.clear();
        self.mods_selected = None;
        self.mods_versions.clear();
        self.mods_versions_loading = false;
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_notice = None;
        self.mods_error = None;
        self.mods_loading = false;
        self.mods_deps_loading = false;
        self.mods_rx = None;
        self.mods_scan_rx = None;
        self.mods_installed_loading = false;
        self.mods_updates_loading = false;
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

    /// 从项目详情回到浏览/搜索结果，保留列表与筛选。
    fn clear_mods_selection(&mut self) {
        self.mods_selected = None;
        self.mods_versions.clear();
        self.mods_versions_loading = false;
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_notice = None;
        self.mods_error = None;
        self.mods_deps_loading = false;
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
        let loader = loader_state_files(&self.root, &row, &game_dir);
        if let Err(e) = rollback::backup_extra(&game_dir, "导入本地文件前", &loader) {
            self.error = Some(format!("无法创建恢复点: {e}"));
            return;
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
        self.home_cache = None;
        self.version_cache = None;
        let Some(game_dir) = self.mods_game_dir.clone() else {
            return;
        };
        self.mods_installed_loading = true;
        self.mods_error = None;
        let (tx, rx) = mpsc::channel();
        self.mods_scan_rx = Some(rx);
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
        self.mods_scan_rx = Some(rx);
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
        let loader_files = loader_state_files(&self.root, &row, &game_dir);
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

    fn start_mods_search(&mut self) {
        self.start_mods_search_at(0);
    }

    fn start_mods_search_more(&mut self) {
        let offset = self.mods_results.len();
        self.start_mods_search_at(offset);
    }

    fn start_mods_search_at(&mut self, offset: usize) {
        if self.busy.is_none() {
            cancel::reset();
        }
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
        self.mods_delete_confirm = None;
        self.mods_versions.clear();
        self.mods_versions_loading = false;
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_offset = 0;
    }

    /// 按当前品类拉两排内容：热门 + 最新。
    fn start_browse(&mut self) {
        if self.busy.is_none() {
            cancel::reset();
        }
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
        if self.busy.is_none() {
            cancel::reset();
        }
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

    /// 一键安装前先把清单摆出来：装到哪、写什么、是否会动恢复点。
    fn review_quick_install(&mut self, project: mods::Project) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.mods_row.clone() else {
            self.mods_error = Some("先在右上角「安装到」选一个版本，再点 +".into());
            return;
        };
        if self.mods_kind == mods::ContentKind::Mod && mod_loader(row.loader).is_none() {
            self.mods_error = Some("原版不能装 Mod，先装 Fabric / NeoForge / Forge".into());
            return;
        }
        let Some(path) = self.mods_game_dir.clone() else {
            self.mods_error = Some("目标目录无效".into());
            return;
        };
        self.pending_install = Some(PendingInstall {
            review: InstallReview {
                name: project.name.clone(),
                kind: self.mods_kind,
                target: target_label(&row),
                path: path.display().to_string(),
                version: "最新兼容版本 · 自动解析前置".into(),
                size: None,
                dependencies: Vec::new(),
                quick: true,
            },
            request: InstallRequest::Quick(project),
        });
    }

    /// 详情页的安装同样先确认，尤其是前置和可选依赖都选好之后。
    fn review_detail_install(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(project) = self.mods_selected.clone() else {
            return;
        };
        let Some(version) = self.mods_version.clone() else {
            return;
        };
        let Some(row) = self.mods_row.clone() else {
            self.mods_error = Some("先在「安装到」选一个版本".into());
            return;
        };
        let Some(path) = self.mods_game_dir.clone() else {
            self.mods_error = Some("目标目录无效".into());
            return;
        };
        let mut dependencies = Vec::new();
        let mut size = version.size.unwrap_or(0);
        for choice in &self.mods_choices {
            let key = (
                choice.dependency.source,
                choice.dependency.project_id.clone(),
            );
            let required = choice.dependency.kind == mods::DependencyKind::Required;
            let optional = choice.dependency.kind == mods::DependencyKind::Optional;
            if !required && !(optional && self.mods_optional.contains(&key)) {
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
                kind: self.mods_kind,
                target: target_label(&row),
                path: path.display().to_string(),
                version: format!(
                    "{} · {}",
                    version.version_number,
                    short_text(&version.file_name, 42)
                ),
                size: (size > 0).then_some(size),
                dependencies,
                quick: false,
            },
            request: InstallRequest::Detail,
        });
    }

    /// 一键安装：不选版本、不点详情，直接装最新兼容版 + 必需前置。
    fn quick_install(&mut self, project: mods::Project) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.mods_row.clone() else {
            self.mods_error = Some("先在右上角「安装到」选一个版本，再点 +".into());
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

    fn select_mods_project(&mut self, project: mods::Project) {
        self.mods_loading = false;
        self.mods_browse_loading = false;
        self.mods_selected = Some(project.clone());
        self.mods_versions.clear();
        self.mods_versions_loading = false;
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_deps_loading = false;
        self.mods_error = None;
        self.mods_notice = None;
        let project_key = format!("{:?}:{}", project.source, project.id);
        let mc = self
            .mods_mc
            .clone()
            .or_else(|| {
                self.catalog
                    .as_ref()
                    .and_then(|catalog| catalog.minecraft.first().cloned())
            })
            .unwrap_or_else(|| "1.21.1".to_string());
        self.mods_versions_loading = true;
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
            self.mods_error = Some("先在右上角「安装到」选一个版本，再安装内容".into());
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
        let rollback_dir = game_dir.clone();
        let rollback_reason = format!("安装 {} 前", version.file_name);
        let loader_files = loader_state_files(&self.root, &row, &game_dir);
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
        if let Some(rx) = &self.rollback_rx {
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
        if let Some(rx) = &self.mods_scan_rx {
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
                    cancel::reset();
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
                    cancel::reset();
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
                    cancel::reset();
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
                    self.mods_versions_loading = false;
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
                    cancel::reset();
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
                    cancel::reset();
                    self.progress = None;
                    self.rx = None;
                    self.mods_quick = None;
                    self.mods_error = Some(e);
                }
                Msg::ModsCancelled => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.progress = None;
                    self.rx = None;
                    self.mods_quick = None;
                    self.mods_notice = Some("Mod 下载已取消".into());
                    self.mods_error = None;
                }
                Msg::ModsInstalled(inst) => {
                    self.mods_installed_loading = false;
                    self.mods_scan_rx = None;
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
                    self.mods_scan_rx = None;
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
                    if self.auto_backup_running {
                        self.auto_backup_running = false;
                        self.backup_cfg.last_run = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or_default();
                        let _ = snapshots::save_config(&self.root, &self.backup_cfg);
                    }
                    match result {
                        Ok(note) => {
                            self.saves_notice = Some(note);
                            self.saves_error = None;
                            self.reload_saves();
                        }
                        Err(e) => self.saves_error = Some(e),
                    }
                }
                Msg::RollbackDone(result) => {
                    self.rollback_busy = false;
                    self.rollback_rx = None;
                    match result {
                        Ok(note) => {
                            self.rollback_notice = Some(note);
                            self.rollback_error = None;
                            self.reload_rollback_points();
                            self.refresh_installed_keys();
                            self.start_mods_scan();
                        }
                        Err(e) => self.rollback_error = Some(e),
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
                            self.log.push(format!("登录失败: {e}"));
                            self.error = Some(friendly_login_error(&e));
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

    /// 当前侧边栏应该高亮哪一项。安装页属于「版本」的子流程。
    fn nav_active(&self) -> Nav {
        if matches!(self.busy, Some(Task::Install)) {
            Nav::Versions
        } else if self.show_mods {
            Nav::Content
        } else if self.show_saves {
            Nav::Saves
        } else if self.show_manage || self.show_install {
            Nav::Versions
        } else {
            Nav::Home
        }
    }

    fn nav_go(&mut self, nav: Nav) {
        if self.busy.is_some() || self.login_busy {
            return;
        }
        if self.nav_active() == nav {
            // 再点一次当前入口：只做“回到这个入口的默认视图”，不重置数据。
            match nav {
                Nav::Versions if self.show_install => {
                    self.show_install = false;
                    self.show_manage = true;
                    self.repair_target = None;
                }
                Nav::Content if !self.mods_show_installed => self.open_content(true),
                _ => {}
            }
            return;
        }
        match nav {
            Nav::Home => {
                self.show_mods = false;
                self.show_saves = false;
                self.show_manage = false;
                self.show_install = false;
                self.repair_target = None;
                self.error = None;
                self.notice = None;
                // 首页只是切视图：版本列表已在安装/删除/内容变更后刷新过。
                // 不再在这里同步 scan 每个版本的 health，否则点击会卡一下。
                self.ensure_visible_selection();
            }
            Nav::Versions => {
                if self.visible_count() == 0 {
                    self.open_install();
                } else {
                    self.show_mods = false;
                    self.show_saves = false;
                    self.show_install = false;
                    self.show_manage = true;
                    self.repair_target = None;
                    self.delete_confirm = None;
                    self.error = None;
                    self.ensure_visible_selection();
                }
            }
            Nav::Content => self.open_content(true),
            Nav::Saves => self.open_saves_page(),
        }
    }

    /// 内容只有一页：发现和已安装是同一页的两种模式。
    fn open_content(&mut self, installed: bool) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self
            .selected_row()
            .or_else(|| self.visible_rows().next().cloned())
        else {
            self.open_install();
            return;
        };
        self.selected = Some(row.id.clone());
        self.show_saves = false;
        self.show_manage = false;
        self.show_install = false;
        self.error = None;
        self.notice = None;
        let same_target = self
            .mods_row
            .as_ref()
            .is_some_and(|current| current.id == row.id);
        if !self.show_mods || !same_target {
            self.enter_mods(Some(row));
        }
        self.clear_mods_selection();
        self.mods_show_installed = installed;
        self.mods_filter.clear();
        self.mods_delete_confirm = None;
        self.mods_error = None;
        self.mods_notice = None;
        if installed {
            if self.mods_game_dir.is_some() {
                self.start_mods_scan();
            }
        } else {
            self.mods_searching = false;
            self.mods_shelf = None;
            self.mods_query.clear();
            if self.mods_browse.is_empty() && !self.mods_browse_loading {
                self.start_browse();
            }
        }
    }

    fn open_saves_page(&mut self) {
        if self.busy.is_some() || self.login_busy {
            return;
        }
        let Some(row) = self
            .selected_row()
            .or_else(|| self.visible_rows().next().cloned())
        else {
            self.open_install();
            return;
        };
        self.selected = Some(row.id.clone());
        self.open_saves(row);
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let wide = ui.ctx().content_rect().width() >= 1080.0;
        let width = if wide { 204.0 } else { 76.0 };
        let active = self.nav_active();
        let current = self.selected_row().or_else(|| {
            self.visible_rows()
                .min_by_key(|row| (row.loader.rank(), row.mc.clone()))
                .cloned()
        });
        let versions = self.visible_count();
        let account_name = self
            .account
            .as_ref()
            .map(|account| account.name.clone())
            .unwrap_or_else(|| self.username.clone());
        let linked = self.account.is_some();
        let busy = self.busy.is_some();
        let login_busy = self.login_busy;
        let enabled = !busy && !login_busy;
        let mut go: Option<Nav> = None;
        let mut account_clicked = false;
        let mut settings_clicked = false;

        let panel = egui::Panel::left("shell_nav")
            .resizable(false)
            .exact_size(width)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin {
                left: if wide { 14 } else { 10 },
                right: if wide { 14 } else { 10 },
                top: 18,
                bottom: 14,
            }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if wide {
                    ui.horizontal(|ui| {
                        logo_mark(ui);
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(
                                egui::RichText::new("nailong")
                                    .size(19.0)
                                    .family(bold_family())
                                    .color(ACCENT_TEXT),
                            );
                            ui.label(
                                egui::RichText::new("Minecraft 启动器")
                                    .size(8.0)
                                    .color(MUTED),
                            );
                        });
                    });
                } else {
                    ui.vertical_centered(logo_mark);
                }
                ui.add_space(if wide { 20.0 } else { 18.0 });

                if wide {
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 66.0),
                        egui::Sense::click(),
                    );
                    ui.painter().rect(
                        rect,
                        11,
                        CARD,
                        egui::Stroke::new(1.0, if response.hovered() { ACCENT_HI } else { LINE }),
                        egui::StrokeKind::Inside,
                    );
                    let inner = rect.shrink2(egui::vec2(12.0, 9.0));
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(inner)
                            .layout(egui::Layout::top_down(egui::Align::LEFT)),
                    );
                    child.spacing_mut().item_spacing.y = 1.0;
                    if let Some(row) = &current {
                        child.horizontal(|ui| {
                            status_dot(ui, row.health.missing == 0);
                            ui.add_space(1.0);
                            ui.label(egui::RichText::new("当前版本").size(8.5).color(MUTED));
                        });
                        let headline = if row.instance.is_some() {
                            row.label.clone()
                        } else {
                            row.mc.clone()
                        };
                        let detail = match &row.loader_version {
                            Some(version) => format!("{} {version}", row.loader.name()),
                            None => row.loader.name().to_string(),
                        };
                        child.add(
                            egui::Label::new(
                                egui::RichText::new(headline)
                                    .size(13.5)
                                    .family(bold_family()),
                            )
                            .truncate(),
                        );
                        child.add(
                            egui::Label::new(egui::RichText::new(detail).size(9.0).color(MUTED))
                                .truncate(),
                        );
                    } else {
                        child.add_space(2.0);
                        child.label(
                            egui::RichText::new("还没有版本")
                                .size(12.0)
                                .family(bold_family()),
                        );
                        child.label(egui::RichText::new("点击安装").size(8.5).color(ACCENT_TEXT));
                    }
                    let response = response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("打开版本管理");
                    if response.clicked() {
                        go = Some(Nav::Versions);
                    }
                    ui.add_space(16.0);
                }

                for nav in Nav::ALL {
                    let response = sidebar_item(ui, nav, active == nav, wide, enabled);
                    if response.clicked() {
                        go = Some(nav);
                    }
                    ui.add_space(3.0);
                }

                // Footer is placed with an explicit spacer: a nested
                // bottom_up layout can shrink to its content and overlap the
                // nav list in a side panel.
                let footer = if wide { 106.0 } else { 92.0 };
                ui.add_space((ui.available_height() - footer).max(10.0));
                if busy || login_busy {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), 34.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if wide {
                                ui.spinner();
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(if login_busy {
                                        "正在登录…"
                                    } else {
                                        "任务进行中"
                                    })
                                    .size(9.5)
                                    .color(MUTED),
                                );
                            } else {
                                ui.vertical_centered(|ui| {
                                    ui.spinner();
                                });
                            }
                        },
                    );
                } else {
                    let response = sidebar_account(ui, &account_name, linked, wide);
                    if response.clicked() {
                        account_clicked = true;
                    }
                    if sidebar_settings(ui, wide, true).clicked() {
                        settings_clicked = true;
                    }
                }
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(8.0)
                            .color(MUTED),
                    );
                    if wide {
                        ui.label(
                            egui::RichText::new(format!("{versions} 个版本"))
                                .size(8.0)
                                .color(MUTED),
                        );
                    }
                });
            });

        let rect = panel.response.rect;
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom()],
            egui::Stroke::new(1.0, LINE),
        );

        if let Some(nav) = go {
            self.nav_go(nav);
        }
        if account_clicked {
            if self.account.is_some() {
                self.show_account = true;
            } else {
                self.start_login();
            }
        }
        if settings_clicked {
            self.show_account = false;
            self.show_settings = true;
        }
    }

    /// 首次启动：把 Java、目录和第一个版本排成三步，别一上来就甩表单。
    fn welcome_view(&mut self, ui: &mut egui::Ui) {
        let width = (ui.available_width() - 48.0).clamp(320.0, 720.0);
        let pad = ((ui.available_width() - width) / 2.0).max(0.0);
        let content_w = width - 48.0;
        let mut install = false;
        let mut import_pack = false;
        let mut recheck = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(46.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            motion::accent_bar(ui, 36.0);
                            ui.add_space(11.0);
                            ui.label(
                                egui::RichText::new("欢迎使用 nailong")
                                    .size(31.0)
                                    .family(bold_family()),
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("装得干净，坏了能救。")
                                    .size(13.0)
                                    .family(bold_family())
                                    .color(ACCENT_TEXT),
                            );
                            ui.label(
                                egui::RichText::new("第一次启动，只需要把这三件事准备好。")
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        });
                        ui.add_space(20.0);
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(16)
                            .shadow(motion::card_shadow())
                            .inner_margin(egui::Margin::symmetric(24, 20))
                            .show(ui, |ui| {
                                ui.set_width(content_w);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("开始之前")
                                            .size(17.0)
                                            .family(bold_family()),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("大约 1 分钟")
                                                    .size(9.5)
                                                    .color(MUTED),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(10.0);
                                ui.separator();
                                ui.add_space(12.0);

                                // 第一步：Java。没有 Java 不拦着下载，但先把路说清楚。
                                ui.horizontal_top(|ui| {
                                    onboarding_badge(ui, "1", self.java_ok);
                                    ui.add_space(10.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(content_w - 236.0, 1.0),
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                            ui.set_width(content_w - 236.0);
                                            ui.label(
                                                egui::RichText::new("确认 Java")
                                                    .size(14.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(
                                                    "启动游戏需要 Java 21 或更高；下载版本本身不需要。",
                                                )
                                                .size(10.0)
                                                .color(MUTED),
                                            );
                                            if let Some(label) = &self.java_label {
                                                ui.add_space(3.0);
                                                ui.label(
                                                    egui::RichText::new(short_text(label, 46))
                                                        .size(9.5)
                                                        .color(OK),
                                                )
                                                .on_hover_text(label);
                                            }
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.vertical(|ui| {
                                                if self.java_ok {
                                                    ui.horizontal(|ui| {
                                                        status_dot(ui, true);
                                                        ui.label(
                                                            egui::RichText::new("已就绪")
                                                                .size(10.5)
                                                                .color(OK),
                                                        );
                                                    });
                                                } else {
                                                    ui.label(
                                                        egui::RichText::new("未找到合适的 Java")
                                                            .size(10.5)
                                                            .color(WARN),
                                                    );
                                                    ui.add_space(5.0);
                                                    if motion::secondary_sized(
                                                        ui,
                                                        "打开 Java 下载页",
                                                        true,
                                                        146.0,
                                                        30.0,
                                                        10.5,
                                                    )
                                                    .clicked()
                                                    {
                                                        open_url(
                                                            "https://adoptium.net/temurin/releases/?version=21",
                                                        );
                                                    }
                                                    if action_link(ui, "重新检测").clicked() {
                                                        recheck = true;
                                                    }
                                                }
                                            });
                                        },
                                    );
                                });
                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(12.0);

                                // 第二步：目录。玩家只需要知道东西放在哪。
                                ui.horizontal_top(|ui| {
                                    onboarding_badge(ui, "2", true);
                                    ui.add_space(10.0);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(content_w - 236.0, 1.0),
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                            ui.set_width(content_w - 236.0);
                                            ui.label(
                                                egui::RichText::new("确认游戏目录")
                                                    .size(14.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(
                                                    "版本、Mod 和存档都在这里，不写系统目录。",
                                                )
                                                .size(10.0)
                                                .color(MUTED),
                                            );
                                        },
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.vertical(|ui| {
                                                let path = self.root.dir().display().to_string();
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(short_text(&path, 34))
                                                            .size(9.5)
                                                            .color(MUTED),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(&path);
                                                if action_link(ui, "打开目录").clicked() {
                                                    open_folder(self.root.dir());
                                                }
                                            });
                                        },
                                    );
                                });
                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(12.0);

                                // 第三步：只留一个主行动，整合包是次选。
                                ui.horizontal_top(|ui| {
                                    onboarding_badge(ui, "3", false);
                                    ui.add_space(10.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("装第一个游戏配置")
                                                .size(14.0)
                                                .family(bold_family()),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "原版 / Fabric / NeoForge 都可以；整合包会顺带装好加载器和 Mod。",
                                            )
                                            .size(10.0)
                                            .color(MUTED),
                                        );
                                    });
                                });
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    if motion::primary_sized_glyph(
                                        ui,
                                        "安装第一个版本",
                                        true,
                                        198.0,
                                        40.0,
                                        13.0,
                                        motion::PrimaryGlyph::Download,
                                    ) {
                                        install = true;
                                    }
                                    if motion::secondary_sized(
                                        ui,
                                        "导入整合包",
                                        true,
                                        112.0,
                                        38.0,
                                        11.0,
                                    )
                                    .clicked()
                                    {
                                        import_pack = true;
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(
                                                    "装完就能在启动台点开始",
                                                )
                                                .size(9.5)
                                                .color(MUTED),
                                            );
                                        },
                                    );
                                });
                            });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            status_dot(ui, true);
                            ui.label(
                                egui::RichText::new(
                                    "下载会校验完整性；内容变更前自动创建恢复点。Java 缺失时也能先装版本。",
                                )
                                .size(9.5)
                                .color(MUTED),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "v{}",
                                            env!("CARGO_PKG_VERSION")
                                        ))
                                        .size(9.0)
                                        .color(MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(24.0);
                    });
                });
            });
        if recheck {
            let runtimes = java::find_all();
            self.java_ok = runtimes.iter().any(|runtime| runtime.major >= 21);
            self.java_label = runtimes
                .iter()
                .find(|runtime| runtime.major >= 21)
                .map(|runtime| runtime.label());
        }
        if install {
            self.open_install();
        }
        if import_pack && let Some(path) = pick_modpack() {
            self.start_modpack(path);
        }
    }

    fn main_view(&mut self, ui: &mut egui::Ui) {
        let rows: Vec<Row> = self.visible_rows().cloned().collect();
        let Some(row) = self
            .selected_row()
            .filter(|r| self.is_visible(r))
            .or_else(|| rows.first().cloned())
        else {
            self.welcome_view(ui);
            return;
        };
        let width = (ui.available_width() - 48.0).clamp(320.0, 1000.0);
        let pad = ((ui.available_width() - width) / 2.0).max(0.0);
        let game_dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        let cache = self.home_cache_for(&row, &game_dir);
        let latest_world = cache.latest_world.clone();
        let (mods, packs, _shaders, world_count) =
            (cache.mods, cache.packs, cache.shaders, cache.worlds);
        let preview = cache.preview.clone();
        let mut chosen: Option<String> = None;
        let mut launch = false;
        let mut repair = false;
        let mut open_manage = false;
        let mut open_saves = false;
        let mut open_content_installed = false;
        let mut open_content_discover = false;
        let mut open_dir: Option<PathBuf> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(26.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.horizontal(|ui| {
                            motion::accent_bar(ui, 24.0);
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("启动台")
                                    .size(26.0)
                                    .family(bold_family()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{} 个版本", rows.len()))
                                            .size(9.0)
                                            .color(MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(16.0);

                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(18)
                            .shadow(motion::card_shadow())
                            .inner_margin(egui::Margin::symmetric(30, 26))
                            .show(ui, |ui| {
                                ui.set_width(width - 60.0);
                                let right_w = if width >= 820.0 { 258.0 } else { 218.0 };
                                let left_w = (width - 60.0 - right_w - 24.0).max(300.0);
                                ui.horizontal_top(|ui| {
                                    ui.vertical(|ui| {
                                        ui.set_width(left_w);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new("当前版本")
                                                    .size(10.0)
                                                    .color(MUTED),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    let mut picked = row.id.clone();
                                                    egui::ComboBox::from_id_salt(
                                                        "home_version",
                                                    )
                                                    .selected_text(
                                                        egui::RichText::new("切换")
                                                            .size(10.5),
                                                    )
                                                    .width(124.0)
                                                    .show_ui(ui, |ui| {
                                                        for item in &rows {
                                                            ui.selectable_value(
                                                                &mut picked,
                                                                item.id.clone(),
                                                                &item.label,
                                                            );
                                                        }
                                                    });
                                                    if picked != row.id {
                                                        chosen = Some(picked);
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(4.0);
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&row.label)
                                                    .size(31.0)
                                                    .family(bold_family()),
                                            )
                                            .truncate(),
                                        );
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            status_dot(ui, row.health.missing == 0);
                                            ui.label(
                                                egui::RichText::new(
                                                    if row.health.missing == 0 {
                                                        "安装完整".to_string()
                                                    } else {
                                                        format!(
                                                            "缺 {} 个文件",
                                                            row.health.missing
                                                        )
                                                    },
                                                )
                                                .size(10.5)
                                                .color(if row.health.missing == 0 {
                                                    OK
                                                } else {
                                                    WARN
                                                }),
                                            );
                                            ui.label(
                                                egui::RichText::new("·")
                                                    .size(10.5)
                                                    .color(MUTED),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{mods} Mod · {packs} 资源包 · {world_count} 世界"
                                                ))
                                                .size(10.5)
                                                .color(MUTED),
                                            );
                                        });
                                        ui.add_space(20.0);
                                        ui.horizontal(|ui| {
                                            if row.health.missing > 0 {
                                                if motion::primary_sized_glyph(
                                                    ui,
                                                    "补全资源",
                                                    self.busy.is_none(),
                                                    176.0,
                                                    46.0,
                                                    14.0,
                                                    motion::PrimaryGlyph::Download,
                                                ) {
                                                    repair = true;
                                                }
                                            } else if motion::primary_sized_glyph(
                                                ui,
                                                "开始游戏",
                                                self.busy.is_none(),
                                                176.0,
                                                46.0,
                                                14.5,
                                                motion::PrimaryGlyph::Play,
                                            ) {
                                                launch = true;
                                            }
                                            ui.add_space(8.0);
                                            if motion::secondary_sized(
                                                ui,
                                                "版本详情",
                                                true,
                                                112.0,
                                                46.0,
                                                12.5,
                                            )
                                            .clicked()
                                            {
                                                open_manage = true;
                                            }
                                        });
                                        if let Some(note) = &self.notice {
                                            ui.add_space(8.0);
                                            ui.label(
                                                egui::RichText::new(note).size(10.0).color(OK),
                                            );
                                        }
                                        if let Some(err) = &self.error {
                                            ui.add_space(8.0);
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(err)
                                                        .size(10.0)
                                                        .color(DANGER),
                                                )
                                                .wrap(),
                                            );
                                        }
                                    });
                                    ui.add_space(24.0);
                                    ui.vertical(|ui| {
                                            ui.set_width(right_w);
                                            egui::Frame::new()
                                                .fill(SURFACE2)
                                                .stroke(egui::Stroke::new(1.0, LINE))
                                                .corner_radius(14)
                                                .inner_margin(egui::Margin::symmetric(
                                                    18, 16,
                                                ))
                                                .show(ui, |ui| {
                                                    ui.set_width(right_w - 36.0);
                                                    ui.set_min_height(150.0);
                                                    ui.label(
                                                        egui::RichText::new("最近世界")
                                                            .size(9.5)
                                                            .color(MUTED),
                                                    );
                                                    ui.add_space(7.0);
                                                    if let Some(world) = &latest_world {
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(
                                                                    &world.name,
                                                                )
                                                                .size(16.0)
                                                                .family(bold_family()),
                                                            )
                                                            .truncate(),
                                                        );
                                                        ui.add_space(3.0);
                                                        ui.label(
                                                            egui::RichText::new(format!(
                                                                "{} · {}",
                                                                human_size(world.size),
                                                                human_age(world.modified)
                                                            ))
                                                            .size(9.0)
                                                            .color(MUTED),
                                                        );
                                                        ui.add_space(14.0);
                                                        if motion::secondary_sized(
                                                            ui,
                                                            "打开存档",
                                                            true,
                                                            right_w - 36.0,
                                                            30.0,
                                                            10.5,
                                                        )
                                                        .clicked()
                                                        {
                                                            open_saves = true;
                                                        }
                                                    } else {
                                                        ui.label(
                                                            egui::RichText::new("还没有世界")
                                                                .size(15.0)
                                                                .family(bold_family()),
                                                        );
                                                        ui.add_space(3.0);
                                                        ui.label(
                                                            egui::RichText::new(
                                                                "启动游戏会创建一个新世界",
                                                            )
                                                            .size(9.0)
                                                            .color(MUTED),
                                                        );
                                                        ui.add_space(14.0);
                                                        if motion::secondary_sized(
                                                            ui,
                                                            "打开游戏目录",
                                                            true,
                                                            right_w - 36.0,
                                                            30.0,
                                                            10.5,
                                                        )
                                                        .clicked()
                                                        {
                                                            open_dir =
                                                                Some(game_dir.clone());
                                                        }
                                                    }
                                                    ui.add_space(14.0);
                                                    ui.separator();
                                                    ui.add_space(8.0);
                                                    ui.horizontal(|ui| {
                                                        status_dot(
                                                            ui,
                                                            self.backup_cfg.enabled,
                                                        );
                                                        ui.label(
                                                            egui::RichText::new(
                                                                if self.backup_cfg.enabled {
                                                                    "自动备份已开启"
                                                                } else {
                                                                    "自动备份已关闭"
                                                                },
                                                            )
                                                            .size(8.5)
                                                            .color(MUTED),
                                                        );
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                ui.label(
                                                                    egui::RichText::new(format!(
                                                                        "每 {} 分钟",
                                                                        self.backup_cfg
                                                                            .interval_minutes
                                                                    ))
                                                                    .size(8.0)
                                                                    .color(MUTED),
                                                                );
                                                            },
                                                        );
                                                    });
                                                });
                                        });
                                    });
                            });

                        ui.add_space(18.0);
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(14)
                            .shadow(motion::card_shadow())
                            .inner_margin(egui::Margin::symmetric(20, 16))
                            .show(ui, |ui| {
                                ui.set_width(width - 40.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("最近内容")
                                            .size(12.5)
                                            .family(bold_family()),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if action_link(ui, "全部内容 ›").clicked() {
                                                open_content_installed = true;
                                            }
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                if preview.is_empty() {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("还没有安装内容")
                                                .size(10.5)
                                                .color(MUTED),
                                        );
                                        ui.with_layout(
                                            egui::Layout::right_to_left(
                                                egui::Align::Center,
                                            ),
                                            |ui| {
                                                if action_link(ui, "去发现内容 ›").clicked() {
                                                    open_content_discover = true;
                                                }
                                            },
                                        );
                                    });
                                } else {
                                    for (index, (kind, name, size)) in
                                        preview.iter().enumerate()
                                    {
                                        if index > 0 {
                                            ui.add_space(4.0);
                                            ui.separator();
                                            ui.add_space(4.0);
                                        }
                                        ui.horizontal(|ui| {
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(46.0, 16.0),
                                                egui::Layout::left_to_right(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(kind)
                                                            .size(9.0)
                                                            .color(ACCENT_TEXT),
                                                    );
                                                },
                                            );
                                            ui.label(
                                                egui::RichText::new(name).size(11.5),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(
                                                    egui::Align::Center,
                                                ),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(human_size(*size))
                                                            .size(9.0)
                                                            .color(MUTED),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                }
                            });
                        ui.add_space(28.0);
                    });
                });
            });

        if let Some(id) = chosen {
            self.selected = Some(id);
        }
        if launch {
            self.start_launch();
        }
        if repair {
            self.start_repair(row.clone());
        }
        if open_manage {
            self.nav_go(Nav::Versions);
        }
        if open_saves {
            self.open_saves(row.clone());
        }
        if open_content_installed {
            self.open_content(true);
        }
        if open_content_discover {
            self.open_content(false);
        }
        if let Some(dir) = open_dir {
            open_folder(&dir);
        }
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
                            if self.mods_show_installed {
                                self.installed_view(ui);
                            } else if self.mods_selected.is_some() {
                                self.detail_view(ui);
                            } else if self.mods_searching {
                                self.results_view(ui);
                            } else {
                                self.browse_view(ui);
                            }
                            ui.add_space(24.0);
                        });
                    });
            });
    }

    fn mods_header(&mut self, ui: &mut egui::Ui) {
        let installed_count = self
            .mods_installed
            .iter()
            .filter(|entry| entry.kind == self.mods_kind)
            .count();
        let mut wanted_target: Option<Option<Row>> = None;
        let mut search_go = false;
        ui.horizontal(|ui| {
            motion::accent_bar(ui, 22.0);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("内容").size(26.0).family(bold_family()));
                ui.label(
                    egui::RichText::new("Mod、资源包、光影 · 已安装与发现")
                        .size(10.0)
                        .color(MUTED),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let rows: Vec<Row> = self.visible_rows().cloned().collect();
                let label = self
                    .mods_row
                    .as_ref()
                    .map(|row| row.label.clone())
                    .unwrap_or_else(|| "只浏览".to_string());
                egui::ComboBox::from_id_salt("mods_target")
                    .selected_text(format!("安装到：{label}"))
                    .width(220.0)
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
                if !self.mods_show_installed {
                    let go = ui
                        .add(
                            egui::Button::new(egui::RichText::new("搜索").size(12.0).color(MUTED))
                                .fill(PANEL)
                                .stroke(egui::Stroke::new(1.0, LINE))
                                .corner_radius(8)
                                .min_size(egui::vec2(54.0, 30.0)),
                        )
                        .clicked();
                    let response = text_field(ui, &mut self.mods_query, "名称或关键词", 180.0);
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
            if self.mods_query.trim().is_empty() {
                self.show_browse();
            } else {
                self.start_mods_search();
            }
        }

        ui.add_space(11.0);
        let mut mode: Option<bool> = None;
        let mut wanted_kind: Option<mods::ContentKind> = None;
        ui.horizontal(|ui| {
            if content_tab(ui, "已安装", installed_count, self.mods_show_installed).clicked() {
                mode = Some(true);
            }
            ui.add_space(4.0);
            if content_tab(ui, "发现", 0, !self.mods_show_installed).clicked() {
                mode = Some(false);
            }
            ui.add_space(14.0);
            for candidate in [
                mods::ContentKind::Mod,
                mods::ContentKind::ResourcePack,
                mods::ContentKind::Shader,
            ] {
                let selected = self.mods_kind == candidate;
                let (fill, text, stroke) = if selected {
                    (SURFACE2, ACCENT_TEXT, ACCENT)
                } else {
                    (CARD, MUTED, LINE)
                };
                let button = egui::Button::new(
                    egui::RichText::new(candidate.label())
                        .size(12.5)
                        .family(bold_family())
                        .color(text),
                )
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .corner_radius(9)
                .min_size(egui::vec2(74.0, 32.0));
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
            self.mods_kind = kind;
            self.mods_category = None;
            self.mods_source_filter = None;
            self.mods_shelf = None;
            self.mods_searching = false;
            self.mods_filter.clear();
            self.mods_delete_confirm = None;
            self.mods_notice = None;
            self.mods_error = None;
            self.clear_mods_selection();
            if self.mods_show_installed {
                self.start_mods_scan();
            } else {
                self.start_browse();
            }
        }

        if !self.mods_show_installed && self.mods_row.is_none() {
            let mut wanted_mc: Option<Option<String>> = None;
            let mut wanted_loader: Option<Option<mods::Loader>> = None;
            ui.add_space(8.0);
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
                    egui::RichText::new("选一个目标版本可以一键安装到它；只浏览不会写入磁盘")
                        .size(10.0)
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

    /// 发现 / 已安装是同一个内容库的两种视图。
    fn switch_content_mode(&mut self, installed: bool) {
        if self.mods_show_installed == installed {
            return;
        }
        self.mods_show_installed = installed;
        self.mods_filter.clear();
        self.mods_delete_confirm = None;
        self.mods_notice = None;
        self.mods_error = None;
        self.clear_mods_selection();
        if installed {
            if self.mods_game_dir.is_some() {
                self.start_mods_scan();
            } else {
                self.mods_error = Some("先在上方选一个安装目标".into());
            }
        } else {
            self.show_browse();
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
        self.mods_scan_rx = None;
        self.mods_installed_loading = false;
        self.mods_updates_loading = false;
        self.refresh_installed_keys();
        self.mods_installed.clear();
        if self.mods_show_installed {
            if self.mods_game_dir.is_some() {
                self.start_mods_scan();
            }
        } else {
            self.fetch_cf_categories(self.mods_kind);
            self.start_browse();
        }
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
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(CARD)
                .stroke(egui::Stroke::new(1.0, LINE))
                .corner_radius(14)
                .shadow(motion::card_shadow())
                .inner_margin(egui::Margin::symmetric(20, 18))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spinner();
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
                            .size(9.5)
                            .color(MUTED),
                    );
                    ui.add_space(16.0);
                    for _ in 0..3 {
                        skeleton_line(ui, 12.0);
                        ui.add_space(8.0);
                    }
                });
            return;
        }

        let per_row = if ui.available_width() >= 840.0 {
            4
        } else if ui.available_width() >= 620.0 {
            3
        } else {
            2
        };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(200.0, 300.0);
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
                        if let Some(chosen) = self.project_card(ui, project, card_width, order) {
                            installs.push(chosen);
                        }
                        order += 1;
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
            self.review_quick_install(project);
        }
        if let Some(note) = &self.mods_notice {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(note).size(12.0).color(OK));
        }
    }

    fn results_view(&mut self, ui: &mut egui::Ui) {
        let mut back = false;
        ui.horizontal(|ui| {
            if ghost_button(ui, "‹ 返回").clicked() {
                back = true;
            }
            ui.add_space(10.0);
            match self.mods_shelf {
                Some(sort) => {
                    let title = match sort {
                        mods::Sort::Popular => "热门",
                        mods::Sort::Newest => "最新",
                        _ => "筛选结果",
                    };
                    ui.label(egui::RichText::new(title).size(18.0).family(bold_family()));
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} 个", self.mods_total))
                            .size(10.5)
                            .color(MUTED),
                    );
                }
                None => {
                    ui.label(egui::RichText::new("搜索").size(10.5).color(MUTED));
                    ui.label(
                        egui::RichText::new(format!(
                            "“{}”",
                            short_text(self.mods_query.trim(), 26)
                        ))
                        .size(16.0)
                        .family(bold_family()),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} 个结果", self.mods_total))
                            .size(10.5)
                            .color(MUTED),
                    );
                }
            }
        });
        if back {
            self.show_browse();
            return;
        }
        ui.add_space(4.0);
        for warning in &self.mods_warnings {
            ui.label(egui::RichText::new(warning).size(10.5).color(WARN));
        }
        if self.mods_loading {
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("正在加载…").size(11.0).color(MUTED));
            });
        }
        let per_row = if ui.available_width() >= 840.0 {
            4
        } else if ui.available_width() >= 620.0 {
            3
        } else {
            2
        };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(200.0, 300.0);
        let results = std::mem::take(&mut self.mods_results);
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
        self.mods_results = results;
        for project in installs {
            self.review_quick_install(project);
        }
        if !self.mods_loading && self.mods_results.is_empty() && self.mods_error.is_none() {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(CARD)
                .stroke(egui::Stroke::new(1.0, LINE))
                .corner_radius(14)
                .shadow(motion::card_shadow())
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
                            .size(10.5)
                            .color(MUTED),
                        );
                        ui.add_space(12.0);
                        if primary_wide(ui, "回到浏览", true, 150.0) {
                            self.show_browse();
                        }
                    });
                });
        } else if !self.mods_loading && (self.mods_results.len() as u64) < self.mods_total {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new("加载更多").size(11.5).color(MUTED))
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
        order: usize,
    ) -> Option<mods::Project> {
        #[cfg(not(debug_assertions))]
        let _ = order;
        let key = project_key(project);
        let summary_chars = ((width * 0.2) as usize).clamp(40, 62);
        let installed = self.mods_installed_keys.contains(&key);
        let quick = self.mods_quick.as_deref() == Some(key.as_str());
        let busy = self.busy.is_some();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 112.0), egui::Sense::click());
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
        let hover_t =
            ui.ctx()
                .animate_bool_with_time(response.id.with("card_hover"), hovered, 0.16);
        let lift = hover_t * 3.0;
        let paint = rect.translate(egui::vec2(0.0, -lift));
        let fill = motion::mix(CARD, egui::Color32::from_rgb(255, 245, 228), hover_t);
        let border = motion::mix(LINE, ACCENT, hover_t);
        ui.painter().add(
            egui::epaint::Shadow {
                offset: [0, 10],
                blur: 26,
                spread: 0,
                color: egui::Color32::from_rgba_unmultiplied(
                    120,
                    90,
                    55,
                    (30.0 + 28.0 * hover_t) as u8,
                ),
            }
            .as_shape(paint, 12),
        );
        if hover_t > 0.01 {
            ui.painter().add(
                egui::epaint::Shadow {
                    offset: [0, 2],
                    blur: 22,
                    spread: 0,
                    color: egui::Color32::from_rgba_unmultiplied(
                        245,
                        196,
                        81,
                        (16.0 * hover_t) as u8,
                    ),
                }
                .as_shape(paint, 12),
            );
        }
        ui.painter().rect(
            paint,
            12,
            fill,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(paint.left() + 10.0, paint.top() + 1.0),
                egui::pos2(paint.right() - 10.0, paint.top() + 1.6),
            ),
            1,
            egui::Color32::from_rgba_unmultiplied(240, 224, 203, 200),
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
            if let Some(texture) = self.mods_icons.get(&key) {
                ui.add(
                    egui::Image::from_texture(egui::load::SizedTexture::new(
                        texture.id(),
                        icon_size,
                    ))
                    .corner_radius(9),
                );
            } else {
                let (icon_rect, _) = ui.allocate_exact_size(icon_size, egui::Sense::hover());
                let base = project_color(project);
                motion::gradient_rrect(
                    ui.painter(),
                    icon_rect,
                    9.0,
                    motion::mix(base, egui::Color32::WHITE, 0.14),
                    motion::mix(base, INK, 0.32),
                );
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
                    egui::Color32::WHITE,
                );
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short_text(&project.name, 34))
                        .size(14.0)
                        .family(bold_family()),
                )
                .truncate(),
            );
        });
        child.add(
            egui::Label::new(
                egui::RichText::new(short_text(&project.summary, summary_chars))
                    .size(10.0)
                    .color(MUTED),
            )
            .wrap(),
        );
        child.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} 下载", compact(project.downloads)))
                    .size(9.5)
                    .color(MUTED),
            );
            ui.label(egui::RichText::new("·").size(9.5).color(MUTED));
            ui.label(
                egui::RichText::new(project.source.label())
                    .size(9.5)
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    ui.label(egui::RichText::new("已装").size(10.0).color(OK));
                } else if quick {
                    ui.spinner();
                } else if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(
                            egui::RichText::new("+")
                                .size(15.0)
                                .family(bold_family())
                                .color(ACCENT_TEXT),
                        )
                        .fill(egui::Color32::from_rgb(255, 244, 222))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(232, 178, 92),
                        ))
                        .corner_radius(11)
                        .min_size(egui::vec2(22.0, 22.0)),
                    )
                    .on_hover_text("安装到当前版本")
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

    fn detail_view(&mut self, ui: &mut egui::Ui) {
        let Some(project) = self.mods_selected.clone() else {
            return;
        };
        let key = project_key(&project);
        let installed = self.mods_installed_keys.contains(&key);
        let versions = self.mods_versions.clone();
        let selected = self.mods_version.clone();
        let choices = self.mods_choices.clone();
        let optional = self.mods_optional.clone();
        let deps_loading = self.mods_deps_loading;
        let versions_loading = self.mods_versions_loading;
        let target = self.mods_row.clone();
        let kind = self.mods_kind;
        let mc = self.mods_mc.clone();
        let loader = self.mods_loader;
        let error = self.mods_error.clone();
        let notice = self.mods_notice.clone();
        let busy = self.busy.is_some();
        let catalog = self
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
            ui.label(egui::RichText::new("内容").size(10.5).color(MUTED));
            ui.label(egui::RichText::new("/").size(10.5).color(MUTED));
            ui.label(
                egui::RichText::new(short_text(&project.name, 26))
                    .size(10.5)
                    .color(MUTED),
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
                            if let Some(texture) = self.mods_icons.get(&key) {
                                ui.add(
                                    egui::Image::from_texture(egui::load::SizedTexture::new(
                                        texture.id(),
                                        icon_size,
                                    ))
                                    .corner_radius(12),
                                );
                            } else {
                                let (rect, _) =
                                    ui.allocate_exact_size(icon_size, egui::Sense::hover());
                                let base = project_color(&project);
                                motion::gradient_rrect(
                                    ui.painter(),
                                    rect,
                                    12.0,
                                    motion::mix(base, egui::Color32::WHITE, 0.14),
                                    motion::mix(base, INK, 0.32),
                                );
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
                                    egui::Color32::WHITE,
                                );
                            }
                            ui.add_space(13.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(&project.name)
                                            .size(22.0)
                                            .family(bold_family()),
                                    );
                                    if installed {
                                        ui.add_space(3.0);
                                        egui::Frame::new()
                                            .fill(egui::Color32::from_rgb(234, 246, 238))
                                            .corner_radius(6)
                                            .inner_margin(egui::Margin::symmetric(7, 2))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new("已安装")
                                                        .size(9.0)
                                                        .color(OK),
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
                                    .size(10.5)
                                    .color(MUTED),
                                );
                            });
                        });
                        ui.add_space(11.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&project.summary).size(12.0).color(INK),
                            )
                            .wrap(),
                        );
                        if !categories.is_empty() {
                            ui.add_space(9.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                                for category in &categories {
                                    tag(ui, category, MUTED);
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
                            .fill(PANEL)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(action_w - 28.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("版本").size(9.5).color(MUTED));
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
                                                    .size(9.5)
                                                    .color(MUTED),
                                                ),
                                                None => ui.label(
                                                    egui::RichText::new("只浏览")
                                                        .size(9.5)
                                                        .color(WARN),
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
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new("获取版本…")
                                                .size(10.0)
                                                .color(MUTED),
                                        );
                                    });
                                } else if versions.is_empty() {
                                    ui.label(
                                        egui::RichText::new(if mc.is_none() {
                                            "选定游戏版本后获取文件"
                                        } else {
                                            "没有匹配当前版本的文件"
                                        })
                                        .size(10.5)
                                        .color(if mc.is_none() { MUTED } else { WARN }),
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
                                if let Some(note) = &notice {
                                    ui.add_space(5.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(note).size(10.0).color(OK),
                                        )
                                        .wrap(),
                                    );
                                }
                                if let Some(err) = &error {
                                    ui.add_space(5.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(err).size(10.0).color(DANGER),
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
                                            .size(9.5)
                                            .color(MUTED),
                                        )
                                        .wrap(),
                                    );
                                } else if missing_required {
                                    ui.add_space(6.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new("缺少必需前置，暂时无法安装")
                                                .size(9.5)
                                                .color(WARN),
                                        )
                                        .wrap(),
                                    );
                                } else if incompatible {
                                    ui.add_space(6.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new("这个版本与当前环境不兼容")
                                                .size(9.5)
                                                .color(DANGER),
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
                                    .size(13.0)
                                    .family(bold_family()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("选定版本后自动解析前置")
                                            .size(9.5)
                                            .color(MUTED),
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
                                        tag(ui, game, INK);
                                    }
                                    if version.game_versions.len() > 6 {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "+{}",
                                                version.game_versions.len() - 6
                                            ))
                                            .size(9.0)
                                            .color(MUTED),
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
                                        tag(ui, &loader_label(loader), INK);
                                    }
                                });
                            });
                        } else {
                            ui.label(
                                egui::RichText::new("还没有选中版本")
                                    .size(10.5)
                                    .color(MUTED),
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
                                        .size(10.0)
                                        .color(MUTED),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("必需前置自动安装")
                                            .size(9.5)
                                            .color(MUTED),
                                    );
                                },
                            );
                        });
                        ui.add_space(8.0);
                        if selected.is_none() || versions_loading {
                            ui.label(
                                egui::RichText::new("选好版本后，这里会列出需要的前置。")
                                    .size(10.5)
                                    .color(MUTED),
                            );
                        } else if deps_loading {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(egui::RichText::new("解析前置…").size(10.5).color(MUTED));
                            });
                        } else if choices.is_empty() {
                            ui.horizontal(|ui| {
                                status_dot(ui, true);
                                ui.label(
                                    egui::RichText::new("这个版本没有额外前置")
                                        .size(10.5)
                                        .color(MUTED),
                                );
                            });
                        } else {
                            for (index, choice) in choices.iter().enumerate() {
                                let (label, color, fill) = match choice.dependency.kind {
                                    mods::DependencyKind::Required => (
                                        "必需",
                                        ACCENT_TEXT,
                                        egui::Color32::from_rgb(255, 243, 214),
                                    ),
                                    mods::DependencyKind::Optional => ("可选", MUTED, SURFACE2),
                                    mods::DependencyKind::Incompatible => {
                                        ("不兼容", DANGER, egui::Color32::from_rgb(255, 226, 220))
                                    }
                                    _ => ("内置", MUTED, SURFACE2),
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
                                            egui::RichText::new(short_text(&name, 44)).size(12.0),
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
                                                        .size(9.5)
                                                        .color(MUTED),
                                                ),
                                                None => ui.label(
                                                    egui::RichText::new("缺失")
                                                        .size(9.5)
                                                        .color(WARN),
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
                                    .size(13.0)
                                    .family(bold_family()),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(format!("{} 个", versions.len()))
                                    .size(10.0)
                                    .color(MUTED),
                            );
                        });
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new("点一行切换要安装的版本")
                                .size(9.5)
                                .color(MUTED),
                        );
                        ui.add_space(8.0);
                        if versions.is_empty() {
                            ui.label(
                                egui::RichText::new(if versions_loading {
                                    "正在获取…"
                                } else {
                                    "暂无版本"
                                })
                                .size(10.5)
                                .color(MUTED),
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
                                .size(9.0)
                                .color(MUTED),
                            );
                        }
                    });
                },
            );
        });

        if let Some(mc) = wanted_mc {
            self.mods_mc = Some(mc);
            if self.mods_loader.is_none() && kind == mods::ContentKind::Mod {
                self.mods_loader = Some(mods::Loader::Fabric);
            }
            self.select_mods_project(project.clone());
        }
        if let Some(loader) = wanted_loader {
            self.mods_loader = loader;
            self.select_mods_project(project.clone());
        }
        if let Some(version) = pick
            && selected.as_ref().map(|current| current.id.as_str()) != Some(version.id.as_str())
        {
            self.select_mods_version(version);
        }
        if let Some((key, checked)) = optional_toggle {
            if checked {
                self.mods_optional.insert(key);
            } else {
                self.mods_optional.remove(&key);
            }
        }
        if install {
            self.review_detail_install();
        }
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
        self.saves_selected = None;
        self.reload_saves();
    }

    fn reload_saves(&mut self) {
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        match saves::worlds(&dir) {
            Ok(items) => {
                self.saves_snapshots.clear();
                for world in &items {
                    let items = snapshots::list(&dir, &world.name).unwrap_or_default();
                    self.saves_snapshots.insert(world.name.clone(), items);
                }
                if self
                    .saves_selected
                    .as_ref()
                    .is_none_or(|name| !items.iter().any(|world| &world.name == name))
                {
                    self.saves_selected = items.first().map(|world| world.name.clone());
                }
                self.saves_worlds = items;
            }
            Err(e) => self.saves_error = Some(e.to_string()),
        }
    }

    fn start_snapshot(&mut self, world: String) {
        if self.saves_busy {
            return;
        }
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        let keep = self.backup_cfg.keep;
        self.saves_busy = true;
        self.auto_backup_running = false;
        self.saves_error = None;
        let (tx, rx) = mpsc::channel();
        self.saves_rx = Some(rx);
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

    fn start_restore(&mut self, world: String, id: String) {
        if self.saves_busy {
            return;
        }
        let Some(dir) = self.saves_game_dir() else {
            return;
        };
        self.saves_busy = true;
        self.auto_backup_running = false;
        self.saves_error = None;
        let (tx, rx) = mpsc::channel();
        self.saves_rx = Some(rx);
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

    fn auto_dirs(&self) -> Vec<PathBuf> {
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

    fn maybe_auto_backup(&mut self) {
        if !self.backup_cfg.enabled || self.saves_busy || self.busy.is_some() {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let due = self.backup_cfg.interval_minutes.max(1) * 60;
        if now.saturating_sub(self.backup_cfg.last_run) < due {
            return;
        }
        self.start_auto_backup();
    }

    fn start_auto_backup(&mut self) {
        let dirs = self.auto_dirs();
        let keep = self.backup_cfg.keep;
        self.saves_busy = true;
        self.auto_backup_running = true;
        self.saves_error = None;
        let (tx, rx) = mpsc::channel();
        self.saves_rx = Some(rx);
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
    fn saves_header_controls(
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
        let mut keep = self.backup_cfg.keep;
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
        if keep != self.backup_cfg.keep {
            self.backup_cfg.keep = keep;
            let _ = snapshots::save_config(&self.root, &self.backup_cfg);
        }
        let mut interval = self.backup_cfg.interval_minutes;
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
        if interval != self.backup_cfg.interval_minutes {
            self.backup_cfg.interval_minutes = interval;
            let _ = snapshots::save_config(&self.root, &self.backup_cfg);
        }
        let mut enabled = self.backup_cfg.enabled;
        if ui.checkbox(&mut enabled, "自动备份").changed() {
            self.backup_cfg.enabled = enabled;
            let _ = snapshots::save_config(&self.root, &self.backup_cfg);
        }
    }

    fn saves_view(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self.saves_row.clone() else {
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
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(28, 14)),
            )
            .show(ui, |ui| {
                let wide = ui.available_width() >= 840.0;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            motion::accent_bar(ui, 22.0);
                            ui.add_space(7.0);
                            ui.label(egui::RichText::new("存档").size(26.0).family(bold_family()));
                            ui.add_space(12.0);
                            egui::ComboBox::from_id_salt("saves_target")
                                .selected_text(&row.label)
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
                                    self.saves_worlds.len()
                                )
                            } else {
                                format!(
                                    "{} 个世界 · 快照分块去重，第二次只写改动块",
                                    self.saves_worlds.len()
                                )
                            })
                            .size(10.0)
                            .color(MUTED),
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
                            .size(9.5)
                            .color(MUTED),
                    );
                    ui.add_space(8.0);
                    if action_link(ui, "打开目录").clicked() {
                        open_dir = Some(game_dir.clone());
                    }
                    if action_link(ui, "刷新").clicked() {
                        reload = true;
                    }
                    if self.saves_busy {
                        ui.spinner();
                        ui.label(egui::RichText::new("正在写入快照…").size(10.0).color(MUTED));
                    }
                });
                if let Some(note) = &self.saves_notice {
                    ui.label(egui::RichText::new(note).size(11.0).color(OK));
                }
                if let Some(err) = &self.saves_error {
                    ui.label(egui::RichText::new(err).size(11.0).color(WARN));
                }
            });

        egui::Panel::left("saves_worlds")
            .resizable(false)
            .exact_size(288.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 18)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("世界")
                            .size(12.0)
                            .family(bold_family())
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{}", self.saves_worlds.len()))
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let worlds = self.saves_worlds.clone();
                        for world in &worlds {
                            let active =
                                self.saves_selected.as_deref() == Some(world.name.as_str());
                            let snapshots = self
                                .saves_snapshots
                                .get(&world.name)
                                .map(|items| items.len())
                                .unwrap_or(0);
                            if save_world_row(ui, world, snapshots, active).clicked() {
                                chosen_world = Some(world.name.clone());
                            }
                        }
                        if worlds.is_empty() {
                            ui.add_space(16.0);
                            ui.label(egui::RichText::new("还没有存档").size(11.0).color(MUTED));
                        }
                    });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::symmetric(24, 20)),
            )
            .show(ui, |ui| {
                if self.saves_worlds.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(54.0);
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(16)
                            .shadow(motion::card_shadow())
                            .inner_margin(egui::Margin::symmetric(30, 28))
                            .show(ui, |ui| {
                                ui.set_width(360.0);
                                ui.label(
                                    egui::RichText::new("这个版本还没有存档")
                                        .size(17.0)
                                        .family(bold_family()),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "进游戏创建一个世界，回来刷新就能做快照和回滚。",
                                    )
                                    .size(10.5)
                                    .color(MUTED),
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
                let Some(world) = self.saves_selected.clone() else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(90.0);
                        ui.label(
                            egui::RichText::new("选择左边的世界查看快照")
                                .size(13.0)
                                .color(MUTED),
                        );
                    });
                    return;
                };
                let snapshots = self
                    .saves_snapshots
                    .get(&world)
                    .cloned()
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("快照")
                            .size(13.0)
                            .family(bold_family())
                            .color(MUTED),
                    );
                    ui.label(
                        egui::RichText::new(format!("{} 份", snapshots.len()))
                            .size(10.5)
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::primary_sized(ui, "立即快照", !self.saves_busy, 92.0, 30.0, 11.5)
                        {
                            new_snapshot = Some(world.clone());
                        }
                        if self.saves_confirm.as_deref() == Some(world.as_str()) {
                            if ui.button("取消").clicked() {
                                self.saves_confirm = None;
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("确认删除世界")
                                            .size(11.0)
                                            .color(DANGER),
                                    )
                                    .fill(egui::Color32::from_rgb(255, 226, 220))
                                    .stroke(egui::Stroke::new(1.0, DANGER)),
                                )
                                .clicked()
                            {
                                delete_world = Some(world.clone());
                            }
                        } else if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("删除世界").size(10.5).color(MUTED),
                                )
                                .fill(PANEL)
                                .stroke(egui::Stroke::new(1.0, LINE))
                                .corner_radius(8)
                                .min_size(egui::vec2(78.0, 28.0)),
                            )
                            .clicked()
                        {
                            self.saves_confirm = Some(world.clone());
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
                                self.backup_cfg.enabled,
                                self.backup_cfg.interval_minutes,
                                self.backup_cfg.keep,
                            ) {
                                new_snapshot = Some(world.clone());
                            }
                        }
                        if !snapshots.is_empty() {
                            egui::Frame::new()
                                .fill(CARD)
                                .stroke(egui::Stroke::new(1.0, LINE))
                                .corner_radius(12)
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    for (index, snapshot) in snapshots.iter().enumerate() {
                                        let confirming = self.saves_confirm_backup.as_deref()
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
                                            self.saves_confirm_backup = Some(snapshot.id.clone());
                                        }
                                        if action.cancel_delete {
                                            self.saves_confirm_backup = None;
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
            self.saves_selected = Some(world);
            self.saves_confirm = None;
            self.saves_confirm_backup = None;
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
                    self.saves_notice = Some(format!("已删除存档 {world}"));
                    self.saves_confirm = None;
                    self.reload_saves();
                }
                Err(e) => self.saves_error = Some(e.to_string()),
            }
        }
        if let Some((world, id)) = delete_snapshot {
            match snapshots::delete(&game_dir, &world, &id) {
                Ok(()) => {
                    self.saves_confirm_backup = None;
                    self.reload_saves();
                }
                Err(e) => self.saves_error = Some(e.to_string()),
            }
        }
        if backup_all {
            let dir = game_dir.clone();
            let keep = self.backup_cfg.keep;
            self.saves_busy = true;
            self.auto_backup_running = false;
            let (tx, rx) = mpsc::channel();
            self.saves_rx = Some(rx);
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
            match snapshots::prune(&game_dir, self.backup_cfg.keep) {
                Ok((removed, deleted)) => {
                    self.saves_notice =
                        Some(format!("清理了 {removed} 份旧快照、{deleted} 个无用块"));
                    self.reload_saves();
                }
                Err(e) => self.saves_error = Some(e.to_string()),
            }
        }
        if let Some(row) = wanted_row {
            self.selected = Some(row.id.clone());
            self.open_saves(row);
        }
    }

    fn installed_view(&mut self, ui: &mut egui::Ui) {
        let Some(game_dir) = self.mods_game_dir.clone() else {
            ui.add_space(36.0);
            egui::Frame::new()
                .fill(CARD)
                .stroke(egui::Stroke::new(1.0, LINE))
                .corner_radius(14).shadow(motion::card_shadow())
                .inner_margin(egui::Margin::symmetric(24, 28))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("先选一个安装目标").size(16.0).family(bold_family()),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(
                                "内容需要一个目标版本。在上方「安装到」里选一个，或去「版本」页创建隔离版本。",
                            )
                            .size(11.0)
                            .color(MUTED),
                        );
                        ui.add_space(14.0);
                        if primary_wide(ui, "去版本页", true, 168.0) {
                            self.nav_go(Nav::Versions);
                        }
                    });
                });
            return;
        };

        let kind = self.mods_kind;
        let mut import = false;
        let mut rescan = false;
        let mut check_updates = false;
        let mut go_browse = false;
        let mut update_all: Option<Vec<installed::UpdateCandidate>> = None;
        let mut update_one: Option<installed::UpdateCandidate> = None;
        let mut toggles: Vec<(installed::InstalledEntry, bool)> = Vec::new();
        let mut deletes: Vec<installed::InstalledEntry> = Vec::new();
        let mut open_dir = false;
        let mut open_rollback = false;

        let entries_all: Vec<installed::InstalledEntry> = self
            .mods_installed
            .iter()
            .filter(|entry| entry.kind == kind)
            .cloned()
            .collect();
        let enabled = entries_all.iter().filter(|entry| entry.enabled).count();
        let disabled = entries_all.len().saturating_sub(enabled);
        let filter = self.mods_filter.trim().to_lowercase();
        let entries: Vec<installed::InstalledEntry> = entries_all
            .iter()
            .filter(|entry| {
                filter.is_empty()
                    || entry.name.to_lowercase().contains(&filter)
                    || entry.file.to_lowercase().contains(&filter)
            })
            .cloned()
            .collect();
        let updates: Vec<installed::UpdateCandidate> = self
            .mods_updates
            .iter()
            .filter(|candidate| candidate.entry.kind == kind)
            .cloned()
            .collect();
        let groups = installed::duplicates(&entries_all);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("已安装").size(11.0).color(MUTED));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(entries_all.len().to_string())
                    .size(16.0)
                    .family(bold_family()),
            );
            if disabled > 0 {
                inline_dot(ui);
                ui.label(
                    egui::RichText::new(format!("{disabled} 个已禁用"))
                        .size(10.5)
                        .color(WARN),
                );
            }
            if !filter.is_empty() {
                inline_dot(ui);
                ui.label(
                    egui::RichText::new(format!("筛选出 {}", entries.len()))
                        .size(10.5)
                        .color(MUTED),
                );
            }
            ui.add_space(8.0);
            if motion::secondary_sized(ui, "恢复点", true, 66.0, 24.0, 10.0)
                .on_hover_text("恢复 Mod / 配置到之前的状态")
                .clicked()
            {
                open_rollback = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if accent_outline(ui, "＋ 导入文件").clicked() {
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
                if quick_action(ui, "重新扫描").clicked() {
                    rescan = true;
                }
                ui.add_space(2.0);
                text_field(ui, &mut self.mods_filter, "筛选", 132.0);
            });
        });

        if self.mods_updates_loading {
            ui.add_space(9.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("检查版本更新…").size(10.5).color(MUTED));
            });
        }
        if let Some(note) = &self.mods_notice {
            ui.add_space(9.0);
            ui.add(egui::Label::new(egui::RichText::new(note).size(10.5).color(OK)).wrap());
        }
        if let Some(err) = &self.mods_error {
            ui.add_space(9.0);
            ui.add(egui::Label::new(egui::RichText::new(err).size(10.5).color(DANGER)).wrap());
        }

        let loaded = !self.mods_installed_loading;
        if !loaded {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(CARD)
                .stroke(egui::Stroke::new(1.0, LINE))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(16, 18))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("正在扫描内容目录…")
                                .size(11.5)
                                .color(MUTED),
                        );
                    });
                });
        } else if entries_all.is_empty() {
            ui.add_space(18.0);
            egui::Frame::new()
                .fill(CARD)
                .stroke(egui::Stroke::new(1.0, LINE))
                .corner_radius(14)
                .shadow(motion::card_shadow())
                .inner_margin(egui::Margin::symmetric(24, 30))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(format!("还没有安装 {}", kind.label()))
                                .size(16.0)
                                .family(bold_family()),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "去「发现」挑一个，或者把 {} 文件拖进窗口。",
                                content_ext(kind)
                            ))
                            .size(11.0)
                            .color(MUTED),
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
                    .fill(egui::Color32::from_rgb(255, 244, 214))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgb(232, 178, 92),
                    ))
                    .corner_radius(12)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("↑").size(15.0).color(ACCENT_TEXT));
                            ui.add_space(2.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{} 项有新版本", updates.len()))
                                        .size(12.5)
                                        .family(bold_family()),
                                );
                                ui.label(
                                    egui::RichText::new("逐个更新，旧文件会一并清理")
                                        .size(10.0)
                                        .color(MUTED),
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

            if !groups.is_empty() {
                let names = groups
                    .iter()
                    .map(|(_, items)| {
                        items
                            .iter()
                            .map(|entry| entry.name.clone())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    })
                    .collect::<Vec<_>>()
                    .join("；");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    status_dot(ui, false);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} 组重复安装：{}",
                            groups.len(),
                            short_text(&names, 86)
                        ))
                        .size(10.5)
                        .color(WARN),
                    );
                });
            }

            if entries.is_empty() {
                ui.add_space(18.0);
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(14)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(24, 26))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "没有匹配「{}」的内容",
                                    self.mods_filter.trim()
                                ))
                                .size(14.0)
                                .family(bold_family()),
                            );
                            ui.add_space(8.0);
                            if action_link(ui, "清除筛选").clicked() {
                                self.mods_filter.clear();
                                self.mods_delete_confirm = None;
                            }
                        });
                    });
            } else {
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(14)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        for (index, entry) in entries.iter().enumerate() {
                            let size = content_size(&game_dir, kind, entry);
                            let candidate = updates.iter().find(|c| c.entry.file == entry.file);
                            let confirming =
                                self.mods_delete_confirm.as_deref() == Some(entry.file.as_str());
                            let action = installed_row(
                                ui,
                                entry,
                                size,
                                candidate.map(|c| c.latest.version_number.as_str()),
                                confirming,
                                index + 1 == entries.len(),
                            );
                            if let Some(enabled) = action.toggle {
                                toggles.push((entry.clone(), enabled));
                            }
                            if action.open {
                                open_dir = true;
                            }
                            if action.ask_delete {
                                self.mods_delete_confirm = Some(entry.file.clone());
                            }
                            if action.cancel_delete {
                                self.mods_delete_confirm = None;
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
            .mods_row
            .as_ref()
            .map(|row| loader_state_files(&self.root, row, &game_dir))
            .unwrap_or_default();
        if import && let Some(path) = pick_content_file(kind) {
            if let Err(e) = rollback::backup_extra(&game_dir, "导入内容前", &loader_files) {
                self.mods_error = Some(format!("无法创建恢复点: {e}"));
            } else {
                match installed::import_local(&game_dir, kind, &path) {
                    Ok(entry) => {
                        self.mods_notice = Some(format!("已导入 {}", entry.name));
                        self.mods_error = None;
                        self.start_mods_scan();
                    }
                    Err(e) => self.mods_error = Some(e.to_string()),
                }
            }
        }
        if rescan {
            self.mods_notice = None;
            self.start_mods_scan();
        }
        if check_updates {
            self.start_mods_update_check();
        }
        if open_dir {
            open_folder(&game_dir.join(kind.folder()));
        }
        for (entry, enabled) in toggles {
            let mut copy = entry.clone();
            copy.enabled = !enabled;
            match installed::set_enabled(&game_dir, &mut copy, enabled) {
                Ok(()) => {
                    self.mods_notice = Some(format!(
                        "{} {}",
                        if enabled { "已启用" } else { "已禁用" },
                        entry.name
                    ));
                    self.mods_error = None;
                    self.mods_delete_confirm = None;
                    self.start_mods_scan();
                }
                Err(e) => self.mods_error = Some(e.to_string()),
            }
        }
        for entry in deletes {
            if let Err(e) = rollback::backup_extra(&game_dir, "删除内容前", &loader_files) {
                self.mods_error = Some(format!("无法创建恢复点: {e}"));
                break;
            }
            match installed::remove(&game_dir, &entry) {
                Ok(()) => {
                    self.mods_notice = Some(format!("已删除 {}", entry.name));
                    self.mods_error = None;
                    self.mods_delete_confirm = None;
                    self.start_mods_scan();
                }
                Err(e) => self.mods_error = Some(e.to_string()),
            }
        }
        if let Some(candidates) = update_all {
            self.start_mods_update(candidates);
        } else if let Some(candidate) = update_one {
            self.start_mods_update(vec![candidate]);
        }
        if go_browse {
            self.show_browse();
        }
        if open_rollback {
            self.open_rollback();
        }
    }

    fn version_actions(&mut self, ui: &mut egui::Ui, row: &Row, open_dir: &mut Option<PathBuf>) {
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

    fn manage_view(&mut self, ui: &mut egui::Ui) {
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
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 18)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("版本")
                            .size(12.0)
                            .family(bold_family())
                            .color(MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("＋").size(13.0).color(ACCENT_TEXT),
                                )
                                .fill(PANEL)
                                .stroke(egui::Stroke::new(1.0, LINE))
                                .corner_radius(6)
                                .min_size(egui::vec2(24.0, 22.0)),
                            )
                            .on_hover_text("安装新版本")
                            .clicked()
                        {
                            go_install = true;
                        }
                        ui.label(
                            egui::RichText::new(format!("{}", rows.len()))
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
                });
                ui.add_space(10.0);
                let mut show_base = self.show_base;
                if ui.checkbox(&mut show_base, "显示基础版本").changed() {
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
                            .size(10.0)
                            .color(egui::Color32::from_rgb(118, 126, 140)),
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
                                .size(20.0)
                                .family(bold_family()),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("装一个原版或加载器版本，再回来管理内容与存档")
                                .size(12.0)
                                .color(MUTED),
                        );
                        ui.add_space(16.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("安装新版本")
                                        .size(13.0)
                                        .family(bold_family())
                                        .color(INK),
                                )
                                .fill(ACCENT)
                                .corner_radius(10)
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
                        egui::vec2(title_w, 44.0),
                        egui::Layout::top_down(egui::Align::LEFT),
                        |ui| {
                            ui.horizontal(|ui| {
                                motion::accent_bar(ui, 21.0);
                                ui.add_space(7.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&row.label).size(24.0).family(bold_family()),
                                    )
                                    .truncate(),
                                );
                            });
                            ui.add_space(3.0);
                            ui.horizontal(|ui| {
                                status_dot(ui, row.health.missing == 0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {}",
                                        row.mc,
                                        row.loader.name()
                                    ))
                                    .size(11.0)
                                    .color(MUTED),
                                );
                                ui.label(egui::RichText::new("·").size(11.0).color(MUTED));
                                ui.label(
                                    egui::RichText::new(if row.health.missing == 0 {
                                        "安装完整".to_string()
                                    } else {
                                        format!("缺 {} 个文件", row.health.missing)
                                    })
                                    .size(11.0)
                                    .color(if row.health.missing == 0 { OK } else { WARN }),
                                );
                                if row.health.missing > 0
                                    && ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("补全").size(10.0),
                                            )
                                            .fill(PANEL)
                                            .stroke(egui::Stroke::new(1.0, LINE))
                                            .corner_radius(6)
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
                        ui.horizontal(|ui| {
                            stat_cell(ui, mods, "Mod");
                            stat_cell(ui, packs, "资源包");
                            stat_cell(ui, shaders, "光影");
                            stat_cell(ui, worlds, "存档");
                        });
                        ui.label(
                            egui::RichText::new(format!(
                                "Java 21 {}  ·  默认内存 {} MB",
                                if self.java_ok {
                                    "已检测到"
                                } else {
                                    "未检测到"
                                },
                                self.memory
                            ))
                            .size(10.0)
                            .color(MUTED),
                        );

                        ui.add_space(18.0);
                        ui.label(
                            egui::RichText::new("安装信息")
                                .size(12.0)
                                .family(bold_family())
                                .color(MUTED),
                        );
                        ui.add_space(8.0);
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(12)
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
                                    ui.label(
                                        egui::RichText::new("游戏目录").size(11.0).color(MUTED),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("打开").size(11.0),
                                                    )
                                                    .fill(PANEL)
                                                    .stroke(egui::Stroke::new(1.0, LINE))
                                                    .corner_radius(7)
                                                    .min_size(egui::vec2(52.0, 24.0)),
                                                )
                                                .clicked()
                                            {
                                                open_dir = Some(dir.clone());
                                            }
                                            ui.label(
                                                egui::RichText::new(dir.display().to_string())
                                                    .size(10.5)
                                                    .color(MUTED),
                                            );
                                        },
                                    );
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
                            .fill(SURFACE2)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(16, 12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("内容与存档")
                                            .size(11.5)
                                            .family(bold_family()),
                                    );
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "在左侧「内容」「存档」里按版本管理，这里只负责启动与维护",
                                        )
                                        .size(10.0)
                                        .color(MUTED),
                                    );
                                });
                            });

                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if self.delete_confirm.as_deref() == Some(row.id.as_str()) {
                                ui.label(
                                    egui::RichText::new(format!("确认删除 {}？", row.label))
                                        .size(10.5)
                                        .color(WARN),
                                );
                                if ui.button("取消").clicked() {
                                    self.delete_confirm = None;
                                }
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("确认删除")
                                                .size(11.0)
                                                .color(DANGER),
                                        )
                                        .fill(egui::Color32::from_rgb(255, 226, 220))
                                        .stroke(egui::Stroke::new(1.0, DANGER)),
                                    )
                                    .clicked()
                                {
                                    delete_target = Some(row.clone());
                                }
                            } else if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("删除这个版本")
                                            .size(10.5)
                                            .color(DANGER),
                                    )
                                    .fill(SURFACE)
                                    .stroke(egui::Stroke::new(1.0, LINE))
                                    .corner_radius(8)
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

    fn home_cache_for(&mut self, row: &Row, game_dir: &std::path::Path) -> HomeCache {
        if let Some(cache) = self.home_cache.as_ref()
            && cache.row_id == row.id
        {
            return cache.clone();
        }
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
        let mut worlds = saves::worlds(game_dir).unwrap_or_default();
        worlds.sort_by_key(|world| world.modified);
        let world_count = worlds.len();
        let latest_world = worlds.pop();
        let preview = self.content_preview(row, 3);
        let cache = HomeCache {
            row_id: row.id.clone(),
            mods: count(mods::ContentKind::Mod),
            packs: count(mods::ContentKind::ResourcePack),
            shaders: count(mods::ContentKind::Shader),
            worlds: world_count,
            latest_world,
            preview,
        };
        self.home_cache = Some(cache.clone());
        cache
    }

    fn content_preview(&self, row: &Row, limit: usize) -> Vec<(String, String, u64)> {
        let dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        let index = installed::load(&dir);
        index
            .entries
            .iter()
            .take(limit)
            .map(|entry| {
                let file = if entry.enabled {
                    entry.file.clone()
                } else {
                    format!("{}.disabled", entry.file)
                };
                let size = std::fs::metadata(dir.join(entry.kind.folder()).join(file))
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                (entry.kind.label().to_string(), entry.name.clone(), size)
            })
            .collect()
    }

    fn version_summary(&mut self, row: &Row) -> (usize, usize, usize, usize) {
        if let Some((id, summary)) = &self.version_cache
            && id == &row.id
        {
            return *summary;
        }
        let summary = self.row_summary(row);
        self.version_cache = Some((row.id.clone(), summary));
        summary
    }

    fn row_summary(&self, row: &Row) -> (usize, usize, usize, usize) {
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

    fn install_view(&mut self, ui: &mut egui::Ui) {
        if let Some(target) = self.repair_target.clone() {
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
            .catalog
            .clone()
            .unwrap_or_else(catalog::Catalog::fallback);
        let mut mc = self.install_mc.clone();
        let mut kind = self.install_kind;
        let mut fv = self.install_fabric.clone();
        let mut nv = self.install_neoforge.clone();
        let mut gv = self.install_forge.clone();
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
                        ui.horizontal(|ui| {
                            motion::accent_bar(ui, 23.0);
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("安装游戏")
                                    .size(26.0)
                                    .family(bold_family()),
                            );
                        });
                        ui.add_space(3.0);
                        ui.label(
                            egui::RichText::new("选择 Minecraft 版本和加载器，下载完整客户端")
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.add_space(18.0);

                        ui.horizontal_top(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(left_w, 1.0),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    ui.set_width(left_w);
                                    egui::Frame::new()
                                        .fill(SURFACE)
                                        .stroke(egui::Stroke::new(1.0, LINE))
                                        .corner_radius(14)
                                        .shadow(motion::card_shadow())
                                        .inner_margin(egui::Margin::symmetric(20, 18))
                                        .show(ui, |ui| {
                                            ui.set_width(left_w - 40.0);
                                            ui.label(
                                                egui::RichText::new("Minecraft 版本")
                                                    .size(10.5)
                                                    .color(MUTED),
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
                                            if mc != self.install_mc {
                                                self.install_mc = mc.clone();
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
                                                    .size(10.5)
                                                    .color(MUTED),
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
                                                    if self.catalog_loading {
                                                        ui.spinner();
                                                        ui.label(
                                                            egui::RichText::new("获取版本…")
                                                                .size(10.5)
                                                                .color(MUTED),
                                                        );
                                                    } else if let Some(err) = &self.catalog_error {
                                                        ui.label(
                                                            egui::RichText::new(err)
                                                                .size(10.5)
                                                                .color(MUTED),
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
                                                    if self.java_ok {
                                                        "已检测到"
                                                    } else {
                                                        "未检测到"
                                                    }
                                                ))
                                                .size(10.0)
                                                .color(MUTED),
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
                                        .fill(SURFACE)
                                        .stroke(egui::Stroke::new(1.0, LINE))
                                        .corner_radius(14)
                                        .shadow(motion::card_shadow())
                                        .inner_margin(egui::Margin::symmetric(18, 18))
                                        .show(ui, |ui| {
                                            ui.set_width(right_w - 36.0);
                                            ui.label(
                                                egui::RichText::new("将安装")
                                                    .size(10.5)
                                                    .color(MUTED),
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
                            .fill(PANEL)
                            .stroke(egui::Stroke::new(1.0, if hovering { ACCENT } else { LINE }))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(18, 12))
                            .show(ui, |ui| {
                                ui.set_width(width - 36.0);
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("导入整合包")
                                                .size(12.0)
                                                .family(bold_family()),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "拖入 .mrpack / .zip，自动安装加载器并解压",
                                            )
                                            .size(10.0)
                                            .color(MUTED),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new("选择文件")
                                                        .fill(PANEL)
                                                        .stroke(egui::Stroke::new(1.0, LINE))
                                                        .corner_radius(8)
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
                            .fill(PANEL)
                            .stroke(egui::Stroke::new(1.0, LINE))
                            .corner_radius(12)
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
                        if !self.java_ok {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("未检测到 Java 21，安装后无法启动")
                                    .size(11.0)
                                    .color(WARN),
                            );
                        }
                        ui.add_space(24.0);
                    });
                });
            });

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
        if go {
            self.install_mc = mc.clone();
            self.start_install_worker(mc, kind, version);
        }
        if back {
            self.show_install = false;
            self.repair_target = None;
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

    fn repair_view(&mut self, ui: &mut egui::Ui, target: RepairTarget) {
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
                ui.label(egui::RichText::new(target.label()).size(11.0).color(MUTED));
                ui.add_space(16.0);
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(14)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(20, 18))
                    .show(ui, |ui| {
                        if let Some(note) = &self.notice {
                            ui.add(
                                egui::Label::new(egui::RichText::new(note).size(11.5).color(OK))
                                    .wrap(),
                            );
                            ui.add_space(12.0);
                        }
                        if let Some(err) = &self.error {
                            ui.add(
                                egui::Label::new(egui::RichText::new(err).size(11.5).color(WARN))
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
                                            .color(INK),
                                    )
                                    .fill(ACCENT)
                                    .corner_radius(10)
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
                                        egui::RichText::new("返回").size(11.0).color(MUTED),
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::NONE),
                                )
                                .clicked()
                            {
                                self.show_install = false;
                                self.repair_target = None;
                                self.error = None;
                                self.show_manage = !self.rows.is_empty();
                                self.ensure_visible_selection();
                            }
                        });
                    });
            });
        });
    }

    fn progress_view(&mut self, ui: &mut egui::Ui) {
        let p = self
            .progress
            .clone()
            .unwrap_or_else(|| Progress::stage("准备安装"));
        let title = if matches!(self.busy, Some(Task::Mods)) {
            "正在下载内容"
        } else {
            "正在安装"
        };
        let width = ui.available_width().min(680.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        let now = ui.input(|input| input.time);
        let shown = ui.ctx().animate_value_with_time(
            egui::Id::unique("progress_value"),
            p.fraction(),
            0.30,
        );
        let mut cancel_clicked = false;
        ui.add_space(150.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(16)
                    .shadow(motion::card_shadow())
                    .inner_margin(egui::Margin::symmetric(26, 22))
                    .show(ui, |ui| {
                        ui.set_width(width - 52.0);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new(title).size(22.0).family(bold_family()),
                                );
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(&self.progress_label)
                                        .size(11.0)
                                        .color(MUTED),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !p.indeterminate {
                                        ui.label(
                                            egui::RichText::new(format!("{:.0}%", shown * 100.0))
                                                .size(26.0)
                                                .family(bold_family())
                                                .color(ACCENT_TEXT),
                                        );
                                    }
                                },
                            );
                        });
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            if p.indeterminate {
                                ui.spinner();
                            }
                            ui.label(egui::RichText::new(&p.stage).size(12.5));
                        });
                        ui.add_space(10.0);
                        motion::progress_bar(ui, shown, p.indeterminate, now);
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let files = if p.files_total > 0 {
                                format!("{} / {} 个文件", p.files_done, p.files_total)
                            } else {
                                format!("{} 个文件", p.files_done)
                            };
                            ui.label(egui::RichText::new(files).size(10.5).color(MUTED));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if p.bytes_total > 0 {
                                        ui.label(
                                            egui::RichText::new(progress_detail(&p))
                                                .size(10.5)
                                                .color(MUTED),
                                        );
                                    }
                                },
                            );
                        });
                        ui.add_space(18.0);
                        ui.separator();
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("可以最小化窗口，下载和 processor 会继续运行")
                                    .size(10.0)
                                    .color(MUTED),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if self.cancel_requested {
                                        ui.add_enabled(false, egui::Button::new("正在取消…"));
                                    } else if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("取消安装")
                                                    .size(11.5)
                                                    .color(DANGER),
                                            )
                                            .fill(PANEL)
                                            .stroke(egui::Stroke::new(1.0, LINE))
                                            .corner_radius(8)
                                            .min_size(egui::vec2(92.0, 30.0)),
                                        )
                                        .clicked()
                                    {
                                        cancel_clicked = true;
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("已完成的部分会保留，下次继续时只补缺失文件")
                        .size(10.0)
                        .color(MUTED),
                );
            });
        });
        if cancel_clicked {
            self.cancel_requested = true;
            cancel::cancel();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        self.drain_icons(ui.ctx());
        if self.backup_cfg.enabled {
            ui.ctx().request_repaint_after(Duration::from_secs(5));
            self.maybe_auto_backup();
        }
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
            || self.mods_scan_rx.is_some()
            || self.rollback_rx.is_some()
            || self.mods_browse_loading
            || !self.mods_icons_pending.is_empty()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let now = ui.ctx().input(|input| input.time);
        let page = self.page_id();
        if page != self.page_key {
            self.page_key = page;
            self.page_enter_at = now;
        }
        let enter = (((now - self.page_enter_at) / 0.18) as f32).clamp(0.0, 1.0);
        let eased = motion::ease_out_cubic(enter);
        if enter < 1.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        // Ambient light is intentionally low-FPS: it keeps the window alive
        // without turning the launcher into a space heater.
        if ui.ctx().input(|input| input.focused) {
            ui.ctx().request_repaint_after(Duration::from_millis(66));
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_HOVER_CYCLE").is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        self.sidebar(ui);
        egui::CentralPanel::default_margins().show(ui, |ui| {
            motion::paint_ambient(ui, now);
            ui.add_space((1.0 - eased) * 10.0);
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
            // A veil is more reliable than per-widget opacity here: nested
            // panels and scroll areas keep their own painters, so fading the
            // whole central area at once reads as one page transition.
            let veil = 1.0 - eased;
            if veil > 0.001 {
                ui.painter().rect_filled(
                    ui.max_rect(),
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(
                        BG.r(),
                        BG.g(),
                        BG.b(),
                        (veil * 245.0) as u8,
                    ),
                );
            }
        });
        self.account_overlay(ui.ctx());
        self.settings_overlay(ui.ctx());
        self.rollback_overlay(ui.ctx());
        self.install_review_overlay(ui.ctx());
    }
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

/// 内容页的「已安装 / 发现」主模式：橙色底、深色字，一眼能看出当前在哪。
fn content_tab(ui: &mut egui::Ui, label: &str, count: usize, active: bool) -> egui::Response {
    let text = if count > 0 {
        format!("{label}  {count}")
    } else {
        label.to_string()
    };
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(12.5)
                .family(bold_family())
                .color(if active { INK } else { MUTED }),
        )
        .fill(if active { ACCENT } else { CARD })
        .stroke(egui::Stroke::new(
            1.0,
            if active { ACCENT_LO } else { LINE },
        ))
        .corner_radius(9)
        .min_size(egui::vec2(if count > 0 { 104.0 } else { 78.0 }, 32.0)),
    )
}

/// 品类胶囊 / 已安装切换用的小按钮。
fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let (fill, color, border) = if selected {
        (egui::Color32::from_rgb(255, 241, 214), ACCENT_TEXT, ACCENT)
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
        format!("{:.1}亿", count as f64 / 100_000_000.0)
    } else if count >= 10_000 {
        format!("{:.1}万", count as f64 / 10_000.0)
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

fn feature(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).size(11.5).family(bold_family()));
        ui.label(egui::RichText::new(detail).size(9.5).color(MUTED));
    });
}

fn loader_tile(
    ui: &mut egui::Ui,
    kind: LoaderKind,
    subtitle: &str,
    selected: bool,
    enabled: bool,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 58.0), egui::Sense::click());
    let fill = if selected { SURFACE2 } else { PANEL };
    let stroke = if selected { ACCENT } else { LINE };
    ui.painter().rect(
        rect,
        10,
        fill,
        egui::Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    let title_color = if !enabled {
        MUTED
    } else if selected {
        ACCENT
    } else {
        INK
    };
    let sub = if enabled && !subtitle.is_empty() {
        subtitle.to_string()
    } else if enabled {
        "内置".to_string()
    } else {
        "不可用".to_string()
    };
    ui.painter().text(
        rect.left_top() + egui::vec2(12.0, 14.0),
        egui::Align2::LEFT_TOP,
        kind.name(),
        egui::FontId::new(
            12.5,
            if selected {
                bold_family()
            } else {
                egui::FontFamily::Proportional
            },
        ),
        title_color,
    );
    ui.painter().text(
        rect.left_top() + egui::vec2(12.0, 34.0),
        egui::Align2::LEFT_TOP,
        sub,
        egui::FontId::proportional(9.5),
        MUTED,
    );
    if selected && enabled {
        motion::glow(
            ui.painter(),
            rect,
            10,
            egui::Color32::from_rgba_unmultiplied(245, 158, 11, 40),
            14,
        );
    }
    response
}

fn quick_action(ui: &mut egui::Ui, text: &str) -> egui::Response {
    motion::secondary_sized(ui, text, true, 94.0, 30.0, 11.0)
}

fn action_link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(10.5).color(MUTED))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE),
    )
}

fn avatar_sized(ui: &mut egui::Ui, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let center = rect.center();
    ui.painter().circle_filled(
        center,
        size / 2.0 + 3.0,
        egui::Color32::from_rgba_unmultiplied(245, 158, 11, 18),
    );
    ui.painter().circle_filled(center, size / 2.0, SURFACE2);
    ui.painter().circle_stroke(
        center,
        size / 2.0 - 0.5,
        egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(245, 158, 11, 80)),
    );
    let letter = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::new(size * 0.44, bold_family()),
        ACCENT,
    );
}

fn status_dot(ui: &mut egui::Ui, ok: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 3.5, if ok { SUCCESS } else { WARN });
}

fn inline_dot(ui: &mut egui::Ui) {
    ui.add_space(7.0);
    ui.label(egui::RichText::new("·").size(10.0).color(MUTED));
    ui.add_space(7.0);
}

fn stat_cell(ui: &mut egui::Ui, value: usize, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(96.0, 38.0),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.label(egui::RichText::new(label).size(10.0).color(MUTED));
            ui.add_space(1.0);
            ui.label(
                egui::RichText::new(value.to_string())
                    .size(17.0)
                    .family(bold_family()),
            );
        },
    );
}

fn info_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(11.0).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).size(11.5));
        });
    });
}

fn save_world_row(
    ui: &mut egui::Ui,
    world: &saves::World,
    snapshots: usize,
    active: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::click());
    if active || response.hovered() {
        ui.painter()
            .rect_filled(rect, 9, if active { SURFACE2 } else { SURFACE });
    }
    if active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 10.0),
                egui::vec2(3.0, rect.height() - 20.0),
            ),
            2,
            ACCENT,
        );
    }
    let inner = rect.shrink2(egui::vec2(12.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.vertical(|ui| {
        ui.label(
            egui::RichText::new(&world.name)
                .size(12.0)
                .family(bold_family()),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} · {} · {} 份快照",
                human_size(world.size),
                human_age(world.modified),
                snapshots
            ))
            .size(9.5)
            .color(MUTED),
        );
    });
    response
}

fn version_row(ui: &mut egui::Ui, row: &Row, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::click());
    if active || response.hovered() {
        ui.painter()
            .rect_filled(rect, 9, if active { SURFACE2 } else { SURFACE });
    }
    if active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 10.0),
                egui::vec2(3.0, rect.height() - 20.0),
            ),
            2,
            ACCENT,
        );
    }
    let inner = rect.shrink2(egui::vec2(12.0, 8.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 10.0;
    status_dot(&mut child, row.health.missing == 0);
    child.vertical(|ui| {
        ui.add_space(1.0);
        ui.label(
            egui::RichText::new(&row.label)
                .size(12.5)
                .family(bold_family()),
        );
        ui.label(
            egui::RichText::new(format!("{} · {}", row.mc, row.loader.name()))
                .size(10.0)
                .color(MUTED),
        );
    });
    response
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, LINE))
        .corner_radius(14)
        .shadow(motion::card_shadow())
        .inner_margin(egui::Margin::same(18))
        .shadow(motion::card_shadow())
}

/// 首次启动的步骤编号：完成的一步用绿色，未完成用暖色。
fn onboarding_badge(ui: &mut egui::Ui, number: &str, done: bool) {
    egui::Frame::new()
        .fill(if done {
            egui::Color32::from_rgb(232, 246, 238)
        } else {
            SURFACE2
        })
        .corner_radius(9)
        .inner_margin(egui::Margin::symmetric(9, 6))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(number)
                    .size(11.0)
                    .family(bold_family())
                    .color(if done { OK } else { ACCENT_TEXT }),
            );
        });
}

/// 加载骨架：不伪装成真实内容，只给一个稳定的占位高度。
fn skeleton_line(ui: &mut egui::Ui, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, height / 2.0, SURFACE2);
}

/// 安装确认里的一行「标签 + 值」。长路径省略显示，悬停可看全。
fn install_review_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [64.0, 16.0],
            egui::Label::new(egui::RichText::new(label).size(10.0).color(MUTED)),
        );
        ui.add(egui::Label::new(egui::RichText::new(value).size(11.0)).truncate())
            .on_hover_text(value);
    });
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
    let time = ui.input(|input| input.time) as f32;
    let pulse = (time * 1.4).sin() * 0.5 + 0.5;
    motion::glow(
        ui.painter(),
        rect,
        8,
        egui::Color32::from_rgba_unmultiplied(245, 196, 81, (26.0 + 20.0 * pulse) as u8),
        16,
    );
    motion::gradient_rrect(ui.painter(), rect, 8.0, ACCENT_HI, ACCENT_LO);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "N",
        egui::FontId::new(17.0, bold_family()),
        INK,
    );
}

/// 侧边栏的一级入口：宽窗口带文字，窄窗口只有图标。
fn sidebar_item(
    ui: &mut egui::Ui,
    nav: Nav,
    active: bool,
    wide: bool,
    enabled: bool,
) -> egui::Response {
    let height = 44.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let hovered = enabled && response.hovered();
    if active {
        ui.painter().rect(
            rect,
            10,
            SURFACE2,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(247, 219, 172)),
            egui::StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(0.0, 10.0),
                egui::vec2(3.0, height - 20.0),
            ),
            2,
            ACCENT,
        );
    } else if hovered {
        ui.painter()
            .rect_filled(rect, 10, egui::Color32::from_rgb(255, 247, 234));
    }
    let icon_color = if !enabled {
        egui::Color32::from_rgb(201, 183, 161)
    } else if active {
        ACCENT_TEXT
    } else {
        egui::Color32::from_rgb(166, 146, 122)
    };
    let icon_rect = if wide {
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 25.0, rect.center().y),
            egui::vec2(17.0, 17.0),
        )
    } else {
        egui::Rect::from_center_size(rect.center(), egui::vec2(19.0, 19.0))
    };
    nav_glyph(ui.painter(), icon_rect, nav, icon_color);
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 44.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            nav.label(),
            egui::FontId::new(
                if active { 13.5 } else { 13.0 },
                if active {
                    bold_family()
                } else {
                    egui::FontFamily::Proportional
                },
            ),
            if !enabled {
                egui::Color32::from_rgb(177, 159, 137)
            } else if active {
                INK
            } else {
                egui::Color32::from_rgb(110, 94, 76)
            },
        );
        if active {
            ui.painter().circle_filled(
                egui::pos2(rect.right() - 13.0, rect.center().y),
                2.5,
                ACCENT,
            );
        }
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response.on_hover_text("任务进行中，完成后再切换")
    }
    .on_hover_text(if wide { nav.hint() } else { nav.label() })
}

fn nav_glyph(ui_painter: &egui::Painter, rect: egui::Rect, nav: Nav, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.6, color);
    let center = rect.center();
    let w = rect.width();
    match nav {
        Nav::Home => {
            ui_painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x, center.y - w * 0.48),
                    egui::pos2(center.x + w * 0.46, center.y - w * 0.06),
                    egui::pos2(center.x - w * 0.46, center.y - w * 0.06),
                ],
                egui::Color32::TRANSPARENT,
                stroke,
            ));
            ui_painter.rect_stroke(
                egui::Rect::from_min_max(
                    egui::pos2(center.x - w * 0.30, center.y - w * 0.06),
                    egui::pos2(center.x + w * 0.30, center.y + w * 0.44),
                ),
                1,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Nav::Versions => {
            for index in 0..3 {
                let y = center.y - w * 0.32 + index as f32 * w * 0.30;
                ui_painter.line_segment(
                    [
                        egui::pos2(center.x - w * 0.42, y),
                        egui::pos2(center.x + w * 0.42, y),
                    ],
                    stroke,
                );
                ui_painter.circle_filled(egui::pos2(center.x - w * 0.20, y), 1.4, color);
            }
        }
        Nav::Content => {
            let side = w * 0.34;
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                ui_painter.rect_stroke(
                    egui::Rect::from_center_size(
                        egui::pos2(center.x + dx * w * 0.23, center.y + dy * w * 0.23),
                        egui::vec2(side, side),
                    ),
                    2,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Nav::Saves => {
            ui_painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x - w * 0.30, center.y - w * 0.42),
                    egui::pos2(center.x + w * 0.30, center.y - w * 0.42),
                    egui::pos2(center.x + w * 0.30, center.y + w * 0.42),
                    egui::pos2(center.x, center.y + w * 0.18),
                    egui::pos2(center.x - w * 0.30, center.y + w * 0.42),
                ],
                egui::Color32::TRANSPARENT,
                stroke,
            ));
        }
    }
}

fn sidebar_account(ui: &mut egui::Ui, name: &str, linked: bool, wide: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 10, egui::Color32::from_rgb(255, 247, 234));
    }
    if wide {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 5.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 8.0;
        avatar_sized(&mut child, name, 26.0);
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(
                egui::Label::new(egui::RichText::new(name).size(11.5).family(bold_family()))
                    .truncate(),
            );
            ui.label(
                egui::RichText::new(if linked {
                    "Microsoft 正版"
                } else {
                    "离线 · 点击登录"
                })
                .size(8.0)
                .color(if linked { OK } else { MUTED }),
            );
        });
    } else {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
        ));
        avatar_sized(&mut child, name, 26.0);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if linked {
            "账户"
        } else {
            "登录 Microsoft 账户"
        })
}

/// 侧边栏底部的工具入口：设置不是一级页面，做成安静的滑杆图标。
fn sidebar_settings(ui: &mut egui::Ui, wide: bool, enabled: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 28.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, 8, egui::Color32::from_rgb(255, 247, 234));
    }
    let color = if enabled {
        egui::Color32::from_rgb(146, 127, 105)
    } else {
        egui::Color32::from_rgb(201, 183, 161)
    };
    let icon = if wide {
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 25.0, rect.center().y),
            egui::vec2(14.0, 14.0),
        )
    } else {
        egui::Rect::from_center_size(rect.center(), egui::vec2(15.0, 15.0))
    };
    let stroke = egui::Stroke::new(1.4, color);
    for index in 0..3 {
        let y = icon.top() + 3.0 + index as f32 * 4.0;
        ui.painter().line_segment(
            [egui::pos2(icon.left(), y), egui::pos2(icon.right(), y)],
            stroke,
        );
        let x = match index {
            0 => icon.left() + 4.0,
            1 => icon.right() - 4.5,
            _ => icon.left() + 7.5,
        };
        ui.painter().circle_filled(egui::pos2(x, y), 1.8, color);
    }
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 44.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "设置",
            egui::FontId::proportional(10.5),
            color,
        );
    }
    if enabled {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("设置：内存、Java、目录与日志")
    } else {
        response.on_hover_text("任务进行中")
    }
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

/// 登录失败给玩家一句能行动的话；原始 AADSTS 留在设置里的日志。
fn friendly_login_error(raw: &str) -> String {
    if raw.contains("AADSTS900144") {
        "登录参数不完整，请更新到最新版本后重试".to_string()
    } else if raw.contains("invalid_client") || raw.contains("AADSTS70002") {
        "启动器的微软登录配置暂时不可用，请稍后重试".to_string()
    } else if raw.contains("未拥有") || raw.contains("Game Pass") {
        raw.to_string()
    } else if raw.contains("已取消") {
        "已取消登录".to_string()
    } else {
        "登录失败，请重试；详细信息已写入日志".to_string()
    }
}

/// Windows 的浏览器启动参数：URL 必须整体作为一个参数，不能经过 shell。
#[cfg(any(target_os = "windows", test))]
fn browser_launch_args(url: &str) -> [String; 2] {
    ["url.dll,FileProtocolHandler".to_string(), url.to_string()]
}

fn open_url(url: &str) {
    // 不要用 `cmd /C start`：cmd 会把 URL 里的 `&` 当命令分隔符，登录链接
    // 从第一个 `&` 就被截断，微软会报 AADSTS900144（scope 丢失）。
    // rundll32 直接接收完整 URL，不经过 shell。
    #[cfg(target_os = "windows")]
    {
        let args = browser_launch_args(url);
        let _ = std::process::Command::new("rundll32").args(args).spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
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

/// 内容页统一列宽：宽窗口里居中，窄窗口里占满。
fn mods_column(ui: &mut egui::Ui, max_width: f32, add: impl FnOnce(&mut egui::Ui)) {
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

fn content_ext(kind: mods::ContentKind) -> &'static str {
    match kind {
        mods::ContentKind::Mod => ".jar",
        mods::ContentKind::ResourcePack | mods::ContentKind::Shader => ".zip",
    }
}

fn content_size(
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

fn blend(from: egui::Color32, to: egui::Color32, t: f32) -> egui::Color32 {
    let channel = |a: u8, b: u8| egui::lerp(a as f32..=b as f32, t).round() as u8;
    egui::Color32::from_rgb(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
    )
}

fn toggle_switch(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(36.0, 20.0), egui::Sense::click());
    let t = ui.ctx().animate_bool_with_time(response.id, on, 0.12);
    ui.painter().rect(
        rect,
        10,
        blend(SURFACE2, ACCENT, t),
        egui::Stroke::new(1.0, blend(LINE, ACCENT, t)),
        egui::StrokeKind::Inside,
    );
    let radius = 7.0;
    let x = egui::lerp(
        (rect.left() + 3.0 + radius)..=(rect.right() - 3.0 - radius),
        t,
    );
    let knob = if t > 0.5 {
        BG
    } else {
        egui::Color32::from_rgb(201, 184, 162)
    };
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), radius, knob);
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
struct RowAction {
    toggle: Option<bool>,
    open: bool,
    ask_delete: bool,
    cancel_delete: bool,
    delete: bool,
    update: bool,
}

fn installed_row(
    ui: &mut egui::Ui,
    entry: &installed::InstalledEntry,
    size: Option<u64>,
    update: Option<&str>,
    confirming: bool,
    last: bool,
) -> RowAction {
    let mut action = RowAction::default();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), egui::Sense::hover());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 10, egui::Color32::from_rgb(255, 247, 234));
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
            let name_color = if entry.enabled {
                INK
            } else {
                egui::Color32::from_rgb(160, 143, 123)
            };
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
                        .size(10.0)
                        .color(MUTED),
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
                egui::Button::new(egui::RichText::new("⋯").size(15.0).color(MUTED))
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(6)
                    .min_size(egui::vec2(26.0, 24.0)),
            );
            let _ = menu.ui(ui, |ui| {
                ui.set_min_width(150.0);
                if ui.button("打开所在目录").clicked() {
                    action.open = true;
                    ui.close();
                }
                if ui
                    .button(egui::RichText::new("删除…").color(DANGER))
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
                            egui::RichText::new("可更新").size(10.0).color(ACCENT_TEXT),
                        )
                        .fill(egui::Color32::from_rgb(255, 243, 214))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgb(232, 178, 92),
                        ))
                        .corner_radius(7)
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
            egui::Stroke::new(1.0, LINE),
        );
    }
    action
}

fn primary_wide(ui: &mut egui::Ui, text: &str, enabled: bool, width: f32) -> bool {
    motion::primary_sized(ui, text, enabled, width, 40.0, 13.5)
}

fn accent_outline(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(11.5)
                .family(bold_family())
                .color(ACCENT_TEXT),
        )
        .fill(egui::Color32::from_rgb(255, 244, 222))
        .stroke(egui::Stroke::new(
            1.0,
            egui::Color32::from_rgb(232, 178, 92),
        ))
        .corner_radius(8)
        .min_size(egui::vec2(88.0, 30.0)),
    )
}

#[derive(Default)]
struct SnapshotAction {
    restore: bool,
    ask_delete: bool,
    cancel_delete: bool,
    delete: bool,
}

fn snapshot_row(
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
        ui.painter()
            .rect_filled(rect, 8, egui::Color32::from_rgb(255, 247, 234));
    }
    if newest {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 2.0, rect.top() + 12.0),
                egui::vec2(2.5, 28.0),
            ),
            1.0,
            ACCENT,
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
                .size(12.5)
                .family(bold_family()),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} 个文件 · {}",
                snapshot.files.len(),
                human_size(snapshot.bytes)
            ))
            .size(9.5)
            .color(MUTED),
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
            egui::Stroke::new(1.0, LINE),
        );
    }
    action
}

fn saves_empty_card(
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
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, LINE))
        .corner_radius(14)
        .shadow(motion::card_shadow())
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
                            .size(10.5)
                            .color(MUTED),
                    )
                    .wrap(),
                );
                ui.add_space(12.0);
                primary_wide(ui, "立即快照", true, 132.0)
            };
            let repo = |ui: &mut egui::Ui| {
                ui.label(egui::RichText::new("仓库").size(10.0).color(MUTED));
                ui.add_space(3.0);
                ui.label(egui::RichText::new(human_size(bytes)).size(12.0));
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(format!("{objects} 个数据块 · backups/"))
                        .size(9.5)
                        .color(MUTED),
                );
            };
            let policy = |ui: &mut egui::Ui| {
                ui.label(egui::RichText::new("备份策略").size(10.0).color(MUTED));
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(if auto {
                        "自动备份已开启"
                    } else {
                        "自动备份已关闭"
                    })
                    .size(12.0),
                );
                ui.add_space(3.0);
                ui.label(
                    egui::RichText::new(format!("每 {interval} 分钟 · 保留 {keep} 份"))
                        .size(9.5)
                        .color(MUTED),
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

fn text_field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, LINE))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            ),
    )
}

fn detail_kv_label(ui: &mut egui::Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(58.0, 16.0), egui::Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(9.5),
        MUTED,
    );
}

fn detail_kv(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        detail_kv_label(ui, label);
        ui.add(egui::Label::new(egui::RichText::new(value).size(10.0).color(INK)).truncate());
    });
}

fn loader_label(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "fabric" => "Fabric".into(),
        "neoforge" => "NeoForge".into(),
        "forge" => "Forge".into(),
        "quilt" => "Quilt".into(),
        other => other.into(),
    }
}

fn tag(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    tag_fill(ui, text, color, SURFACE2);
}

fn tag_fill(ui: &mut egui::Ui, text: &str, color: egui::Color32, fill: egui::Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(9.0).color(color));
        });
}

fn small_accent(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(10.0).color(ACCENT_TEXT))
            .fill(egui::Color32::from_rgb(255, 244, 222))
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb(232, 178, 92),
            ))
            .corner_radius(6)
            .min_size(egui::vec2(44.0, 24.0)),
    )
}

fn tiny_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(10.0).color(MUTED))
            .fill(PANEL)
            .stroke(egui::Stroke::new(1.0, LINE))
            .corner_radius(6)
            .min_size(egui::vec2(44.0, 24.0)),
    )
}

fn danger_tiny(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(10.0)
                .color(egui::Color32::from_rgb(255, 226, 220)),
        )
        .fill(egui::Color32::from_rgb(112, 46, 46))
        .corner_radius(6)
        .min_size(egui::vec2(64.0, 24.0)),
    )
}

fn short_date(text: &str) -> String {
    text.get(..10).unwrap_or(text).to_string()
}

fn version_row_mini(ui: &mut egui::Ui, version: &mods::Version, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            8,
            if active {
                SURFACE2
            } else {
                egui::Color32::from_rgb(255, 247, 234)
            },
        );
    }
    if active {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 1.0, rect.top() + 10.0),
                egui::vec2(2.5, 18.0),
            ),
            1.0,
            ACCENT,
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
            .size(11.5)
            .family(bold_family())
            .color(INK),
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
        ui.label(egui::RichText::new(date).size(9.0).color(MUTED));
    });
    response.on_hover_text(format!("{} · {}", version.version_number, meta.join(" · ")))
}

/// 中文正文与粗体分成两个字体族；`RichText::family(bold_family())` 才是真粗体。
pub(crate) fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

fn install_fonts(ctx: &egui::Context) {
    const REGULAR: [&str; 5] = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    const BOLD: [&str; 6] = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Bold.otf",
        "C:\\Windows\\Fonts\\msyhbd.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    let mut fonts = egui::FontDefinitions::default();
    for path in REGULAR {
        if let Ok(bytes) = std::fs::read(path) {
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
            break;
        }
    }
    let mut bold_family = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for path in BOLD {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "cjk_bold".to_owned(),
                egui::FontData::from_owned(bytes).into(),
            );
            bold_family.insert(0, "cjk_bold".to_owned());
            break;
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(|p| Root::new(PathBuf::from(p)))
        .unwrap_or_else(|| Root::new(Root::default_root()));
    #[cfg(debug_assertions)]
    let size = std::env::var("NAILONG_DEBUG_SIZE")
        .ok()
        .and_then(|value| {
            let (w, h) = value.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1120.0, 720.0]);
    #[cfg(not(debug_assertions))]
    let size = [1120.0, 720.0];
    let options = eframe::NativeOptions {
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([900.0, 620.0])
            .with_title("nailong"),
        ..Default::default()
    };
    eframe::run_native(
        "nailong",
        options,
        Box::new(move |cc| {
            let mut visuals = egui::Visuals::light();
            visuals.override_text_color = Some(INK);
            visuals.panel_fill = BG;
            visuals.window_fill = CARD;
            visuals.extreme_bg_color = egui::Color32::from_rgb(255, 253, 248);
            visuals.window_stroke = egui::Stroke::new(1.0, LINE);
            visuals.window_corner_radius = egui::CornerRadius::same(12);
            visuals.menu_corner_radius = egui::CornerRadius::same(12);
            visuals.popup_shadow = egui::epaint::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 0,
                color: egui::Color32::from_rgba_unmultiplied(120, 90, 55, 32),
            };
            visuals.faint_bg_color = SURFACE2;
            visuals.selection.bg_fill = ACCENT_HI;
            visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT_LO);
            visuals.hyperlink_color = ACCENT_TEXT;
            visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(255, 253, 248);
            visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(255, 250, 242);
            visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
            visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK);
            visuals.widgets.hovered.bg_fill = SURFACE2;
            visuals.widgets.hovered.weak_bg_fill = SURFACE2;
            visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT_HI);
            visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, INK);
            visuals.widgets.active.bg_fill = ACCENT_HI;
            visuals.widgets.active.weak_bg_fill = ACCENT_HI;
            visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_LO);
            visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5, INK);
            visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
            visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, INK);
            visuals.text_cursor.stroke = egui::Stroke::new(2.0, ACCENT_LO);
            cc.egui_ctx.set_visuals(visuals);
            install_fonts(&cc.egui_ctx);
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
    fn windows_browser_arg_keeps_full_oauth_query() {
        let url = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=x&scope=XboxLive.signin%20offline_access&state=abc";
        let args = browser_launch_args(url);
        assert_eq!(args[1], url);
        assert!(args[1].contains("&scope=XboxLive.signin"));
    }

    #[test]
    fn login_errors_are_actionable_not_raw_aadsts() {
        let scope =
            friendly_login_error("微软登录失败：invalid_request（AADSTS900144: scope is missing）");
        assert!(scope.contains("更新"));
        assert!(!scope.contains("AADSTS"));
        assert_eq!(
            friendly_login_error("微软登录失败：invalid_client"),
            "启动器的微软登录配置暂时不可用，请稍后重试"
        );
        assert_eq!(friendly_login_error("已取消登录"), "已取消登录");
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
