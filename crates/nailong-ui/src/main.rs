#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::auth;
use nailong_core::bench;
use nailong_core::install::{catalog, fabric, forge, health, modpack, neoforge, vanilla};
use nailong_core::installed;
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::launchcfg::{self, MemoryPick, VersionLaunch};
use nailong_core::learned;
use nailong_core::mods;
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::rollback;
use nailong_core::saves;
use nailong_core::snapshots;
use nailong_core::tuning;
use nailong_core::{Error, Root, cancel, java};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

mod launchview;
mod logview;
mod motion;
mod rings;
mod shots;
mod theme;

use theme as t;

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
    /// 实测调参：会打开游戏窗口自动跑测试世界。
    Bench,
}

/// 侧边栏的四个一级入口。玩家只需要记住这四个地方。
/// 已安装列表的状态筛选。状态数字本身就是入口：点「3 个已禁用」就只看这 3 个。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StatusFilter {
    All,
    Enabled,
    Disabled,
    Updates,
    Duplicates,
}

impl StatusFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Enabled => "已启用",
            Self::Disabled => "已禁用",
            Self::Updates => "可更新",
            Self::Duplicates => "重复",
        }
    }
}

/// 命令行预览：给人看的逐行写法和能复制的整条命令，或者生成失败的原因。
type CommandPreview = Result<(Vec<String>, String), String>;

/// 启动参数窗口的状态：在改哪个版本、草稿，以及命令行预览。
struct LaunchEdit {
    row_id: String,
    draft: VersionLaunch,
    /// 自定义窗口大小的输入框。
    width: String,
    height: String,
    /// 预览对应的设置；和当前草稿不一样就重新算。
    preview_key: String,
    preview_rx: Option<mpsc::Receiver<CommandPreview>>,
    preview: Option<CommandPreview>,
    copied_at: Option<f64>,
}

/// 拖进来的文件分好类之后要做的事。
#[derive(Default)]
struct DropPlan {
    /// 导入到哪个版本、哪个游戏目录。
    target: Option<(Row, PathBuf)>,
    items: Vec<(PathBuf, mods::ContentKind)>,
    packs: Vec<PathBuf>,
    unknown: Vec<PathBuf>,
}

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
    /// 退出码，以及从游戏输出里认出的原因。
    LaunchDone(i32, Option<String>),
    LaunchFailed(String),
    BenchDone(Result<bench::Store, String>),
    BenchCancelled,
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

/// 更新确认里的一个项目。
#[derive(Clone)]
struct ReviewUpdate {
    name: String,
    from: String,
    to: String,
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
    updates: Vec<ReviewUpdate>,
    quick: bool,
    update: bool,
}

#[derive(Clone)]
enum InstallRequest {
    Quick(mods::Project),
    Detail,
    Update(Vec<installed::UpdateCandidate>),
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
    latest_world: Option<saves::World>,
    /// 每个 Mod 的文件名和是否启用，状态圆环一个点对应一个。
    mod_entries: Vec<(String, String, bool)>,
    snapshots: usize,
    latest_snapshot: Option<u64>,
}

type CfCategoryFetch = (mods::ContentKind, Result<Vec<mods::CurseCategory>, String>);

fn project_key(project: &mods::Project) -> String {
    format!("{:?}:{}", project.source, project.id)
}

/// 一个版本的负载，以及它的实测记录和记录是否仍然有效。
/// 启动时会用的负载和 Java，以及这个版本玩过之后学到的内存。
type TuningEntry = (tuning::Workload, java::JavaRuntime, Option<tuning::Learned>);

struct App {
    root: Root,
    rows: Vec<Row>,
    selected: Option<String>,
    username: String,
    /// 最大堆，MB；None 表示按电脑配置和 Mod 数量自动算。
    memory: Option<u32>,
    gc: tuning::GcChoice,
    /// 设置里填的额外 JVM 参数，原样保存。
    jvm_args: String,
    show_tuning: bool,
    javas: Vec<java::JavaRuntime>,
    machine: tuning::Machine,
    /// 每个版本的负载（Java、加载器、Mod 数）和实测记录（以及它是否仍然有效），
    /// 进页面和内容变化时重算。
    tuning_cache: HashMap<String, Option<TuningEntry>>,
    /// 实测调参的确认弹窗。
    bench_confirm: bool,
    /// 「测一下这台电脑」的结果，所有版本共用。
    bench_store: Option<bench::Store>,
    /// 每个版本单独的启动参数，键是版本行的 id。
    launch_cfgs: BTreeMap<String, VersionLaunch>,
    /// 启动参数窗口。
    launch_edit: Option<LaunchEdit>,
    /// 在启动参数里手动选过、但自动检测没找到的 Java。
    extra_javas: Vec<java::JavaRuntime>,
    /// 启动台截图墙，跟着当前版本的游戏目录。
    shots: Option<shots::Shots>,
    reduce_motion: bool,
    theme_pref: t::Preference,
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
    /// 已安装列表按状态筛：全部、已启用、已禁用、可更新、重复。和搜索可以叠加。
    mods_status: StatusFilter,
    /// 这次导入的文件，列表里标「刚导入」，换页就清掉。
    mods_recent: HashSet<String>,
    /// `mods_installed` 是哪个游戏目录的；换目录时先显示那个目录上次的索引。
    mods_installed_dir: Option<PathBuf>,
    /// 下一帧把焦点交给搜索框（Ctrl+F）。
    focus_search: bool,
    /// 内容目录上次扫描时的样子；窗口重新拿到焦点时比一比，变了就自动重扫。
    mods_dir_sig: u64,
    window_focused: bool,
    /// 拖着文件悬停时的预览：(这些文件, 说明, 能不能导入)。
    drop_preview: Option<(Vec<PathBuf>, String, bool)>,
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
    /// 日志窗口；None 时关着。
    log_view: Option<logview::LogView>,
    /// 最近一次启动的日志在哪、结果如何。
    last_launch: Option<logview::LastLaunch>,
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
    login_error: Option<String>,
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
    /// 启动台在后台核对内容目录：(版本行 id, 结果)。索引可能是旧的或从没扫过。
    home_scan: Option<(String, mpsc::Receiver<Vec<installed::InstalledEntry>>)>,
    /// 这一轮缓存里已经核对过的版本行。
    home_verified: Option<String>,
    version_cache: Option<(String, (usize, usize, usize, usize))>,
    page_key: u8,
    page_enter_at: f64,
    modal_key: u8,
    modal_started: f64,
    rx: Option<mpsc::Receiver<Msg>>,
    catalog_rx: Option<mpsc::Receiver<Msg>>,
    /// 这次启动游戏的开始时间，用来显示已运行时长。
    launch_started: Option<Instant>,
    /// 正在运行的游戏用的是哪个版本；运行期间这个版本的内容和存档不能改。
    running_row: Option<String>,
}

struct UiPrefs {
    username: String,
    memory: Option<u32>,
    gc: tuning::GcChoice,
    jvm_args: String,
    reduce_motion: bool,
    theme: t::Preference,
}

/// 玩家名、内存和外观存在数据目录的 `ui.json`，随设置弹窗关闭落盘。
fn load_ui_prefs(root: &Root) -> UiPrefs {
    let value = std::fs::read(root.dir().join("ui.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let get = |key: &str| value.as_ref().and_then(|value| value.get(key));
    let username = get("username")
        .and_then(|value| value.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Player")
        .to_string();
    // 0.33 及以前没有 "gc"，那时的 4096 是默认值而不是用户选的，迁移成自动。
    let legacy = get("gc").is_none();
    let memory = get("memory")
        .and_then(|value| value.as_u64())
        .filter(|&mb| !(legacy && mb == 4096))
        .map(|mb| mb.clamp(1024, 16384) as u32);
    let gc = get("gc")
        .and_then(|value| value.as_str())
        .map(tuning::GcChoice::from_key)
        .unwrap_or_default();
    let jvm_args = get("jvm_args")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let reduce_motion = get("reduce_motion")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let theme = get("theme")
        .and_then(|value| value.as_str())
        .map(t::Preference::from_key)
        .unwrap_or(t::Preference::System);
    UiPrefs {
        username,
        memory,
        gc,
        jvm_args,
        reduce_motion,
        theme,
    }
}

impl App {
    fn new(root: Root) -> Self {
        let javas = java::find_all();
        let java_ok = javas.iter().any(|j| j.major >= 21);
        let java_label = javas.iter().find(|j| j.major >= 21).map(|j| j.label());
        let account = auth::load(&root);
        let backup_cfg = snapshots::load_config(&root);
        let prefs = load_ui_prefs(&root);
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
            username: prefs.username,
            memory: prefs.memory,
            gc: prefs.gc,
            jvm_args: prefs.jvm_args,
            show_tuning: false,
            javas,
            machine: tuning::Machine::detect(),
            tuning_cache: HashMap::new(),
            bench_confirm: false,
            bench_store: None,
            launch_cfgs: BTreeMap::new(),
            launch_edit: None,
            extra_javas: Vec::new(),
            shots: None,
            reduce_motion: prefs.reduce_motion,
            theme_pref: prefs.theme,
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
            mods_status: StatusFilter::All,
            mods_recent: HashSet::new(),
            focus_search: false,
            mods_installed_dir: None,
            mods_dir_sig: 0,
            window_focused: true,
            drop_preview: None,
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
            log_view: None,
            last_launch: None,
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
            login_error: None,
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
            home_scan: None,
            home_verified: None,
            version_cache: None,
            page_key: 255,
            page_enter_at: 0.0,
            modal_key: 0,
            modal_started: 0.0,
            rx: None,
            catalog_rx: None,
            launch_started: None,
            running_row: None,
        };
        app.launch_cfgs = launchcfg::load_all(&app.root);
        app.refresh();
        app.bench_store = bench::load(&app.root);
        {
            // 0.35 给每个版本复制过一份测试实例，现在用不到了。
            let root = app.root.clone();
            std::thread::spawn(move || bench::cleanup_legacy(&root));
        }
        #[cfg(debug_assertions)]
        if let Ok(theme) = std::env::var("NAILONG_DEBUG_THEME") {
            app.theme_pref = t::Preference::from_key(&theme);
        }
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
                "login_error" => {
                    app.show_account = true;
                    app.login_error = Some("登录参数不完整，请更新到最新版本后重试".into());
                }
                "settings" => {
                    app.show_settings = true;
                }
                "log" => {
                    if let Some(row) = row {
                        app.last_launch = Some(logview::LastLaunch {
                            label: row.label.clone(),
                            log: launch::log_path(&app.root, &row.profile),
                            game_dir: row
                                .game_dir
                                .clone()
                                .unwrap_or_else(|| app.root.dir().to_path_buf()),
                            started: std::time::SystemTime::now() - Duration::from_secs(180),
                            outcome: logview::Outcome::Exited(1),
                        });
                        app.error =
                            Some("游戏异常退出（代码 1）：缺少前置 Mod：日志里写了缺哪个".into());
                        app.open_log();
                    }
                }
                "running" => {
                    app.busy = Some(Task::Launch);
                    app.launch_started = Some(Instant::now());
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
        #[cfg(debug_assertions)]
        if let Some(count) = std::env::var("NAILONG_DEBUG_UPDATES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            let dir = row
                .game_dir
                .clone()
                .unwrap_or_else(|| app.root.version_dir(&row.profile));
            app.mods_updates = installed::load(&dir)
                .entries
                .into_iter()
                .filter(|entry| entry.kind == mods::ContentKind::Mod)
                .take(count)
                .map(|entry| installed::UpdateCandidate {
                    latest: mods::Version {
                        source: mods::Source::Modrinth,
                        kind: mods::ContentKind::Mod,
                        project_id: String::new(),
                        id: String::new(),
                        name: entry.name.clone(),
                        version_number: "debug".into(),
                        game_versions: Vec::new(),
                        loaders: Vec::new(),
                        file_name: entry.file.clone(),
                        download_url: None,
                        sha1: None,
                        size: None,
                        published: None,
                        dependencies: Vec::new(),
                    },
                    entry,
                })
                .collect();
            app.mods_row = Some(row);
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

    /// 安装和下载会占住进度页；游戏运行不会，玩的时候照样能看内容和存档。
    fn page_locked(&self) -> bool {
        matches!(
            self.busy,
            Some(Task::Install) | Some(Task::Mods) | Some(Task::Bench)
        ) || self.login_busy
    }

    /// 现在不能改这个版本的原因；`None` 表示可以改。
    fn write_block(&self, row_id: Option<&str>) -> Option<&'static str> {
        match self.busy {
            Some(Task::Install) | Some(Task::Mods) => Some("正在安装，完成后再试"),
            Some(Task::Bench) => Some("正在测试这台电脑，完成后再试"),
            Some(Task::Launch) if row_id.is_some() && self.running_row.as_deref() == row_id => {
                Some("游戏正在使用这个版本，退出游戏后再修改")
            }
            _ => None,
        }
    }

    /// 页面底部的一行提示：错误、完成通知，以及“查看日志”。
    fn message_bar(&mut self, ui: &mut egui::Ui) {
        let (text, color, is_error) = match (&self.error, &self.notice) {
            (Some(error), _) => (error.clone(), t::danger(), true),
            (None, Some(notice)) => (notice.clone(), t::ok(), false),
            (None, None) => return,
        };
        let full = ui.max_rect();
        let bar = egui::Rect::from_min_max(
            egui::pos2(full.left(), full.bottom() - 40.0),
            full.right_bottom(),
        );
        ui.painter().rect_filled(bar, 0.0, t::surface());
        ui.painter().line_segment(
            [bar.left_top(), bar.right_top()],
            egui::Stroke::new(1.0, t::hairline()),
        );
        let mut close = false;
        let mut show_log = false;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(egui::vec2(24.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(color)).truncate());
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("×").size(15.0)).frame(false))
                .on_hover_text("关闭提示")
                .clicked()
            {
                close = true;
            }
            if is_error
                && ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("查看日志").size(13.0).color(t::sub()),
                        )
                        .frame(false),
                    )
                    .clicked()
            {
                show_log = true;
            }
        });
        if close {
            self.error = None;
            self.notice = None;
        }
        if show_log {
            self.open_log();
        }
    }

    /// 按钮不可用时给出的说明。
    fn busy_reason(&self) -> &'static str {
        match self.busy {
            Some(Task::Launch) => "游戏运行中，退出游戏后再安装",
            Some(Task::Bench) => "正在测试这台电脑，完成后再试",
            _ => "正在安装，完成后再试",
        }
    }

    fn page_id(&self) -> u8 {
        if matches!(
            self.busy,
            Some(Task::Install) | Some(Task::Mods) | Some(Task::Bench)
        ) {
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

    /// 全局快捷键：只在不输入文字、没有弹窗时生效。
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input()
            || self.show_settings
            || self.show_account
            || self.login_busy
            || self.login_url.is_some()
            || self.pending_install.is_some()
            || self.show_rollback
            || self.log_view.is_some()
            || self.launch_edit.is_some()
            || self.shots.as_ref().is_some_and(|shots| shots.viewing())
        {
            return;
        }
        let mut nav = None;
        let mut settings = false;
        let mut refresh = false;
        let mut search = false;
        let mut logs = false;
        ctx.input_mut(|input| {
            for (key, target) in [
                (egui::Key::Num1, Nav::Home),
                (egui::Key::Num2, Nav::Versions),
                (egui::Key::Num3, Nav::Content),
                (egui::Key::Num4, Nav::Saves),
            ] {
                if input.consume_key(egui::Modifiers::CTRL, key) {
                    nav = Some(target);
                }
            }
            settings = input.consume_key(egui::Modifiers::CTRL, egui::Key::Comma);
            refresh = input.consume_key(egui::Modifiers::NONE, egui::Key::F5);
            search = input.consume_key(egui::Modifiers::CTRL, egui::Key::F);
            logs = input.consume_key(egui::Modifiers::CTRL, egui::Key::L);
        });
        if logs {
            self.open_log();
            return;
        }
        if search && self.show_mods {
            self.focus_search = true;
        }
        if settings {
            self.show_account = false;
            self.show_settings = true;
        }
        if refresh {
            self.refresh();
        }
        if let Some(nav) = nav {
            self.nav_go(nav);
        }
    }

    /// 打开日志窗口。游戏异常退出时直接是这次的游戏输出，启动器自己出的错是启动器记录。
    fn open_log(&mut self) {
        let game = match (&self.last_launch, &self.error) {
            (Some(_), Some(error)) => error.starts_with("游戏异常退出"),
            (Some(last), None) => !matches!(last.outcome, logview::Outcome::Failed(_)),
            (None, _) => false,
        };
        self.show_settings = false;
        self.show_account = false;
        self.log_view = Some(logview::LogView::new(if game {
            logview::Tab::Game
        } else {
            logview::Tab::Launcher
        }));
    }

    fn log_overlay(&mut self, ctx: &egui::Context) {
        let Some(view) = &mut self.log_view else {
            return;
        };
        if view.show(ctx, self.last_launch.as_ref(), &self.log) {
            self.log_view = None;
        }
    }

    fn refresh_java(&mut self) {
        let runtimes = java::find_all();
        self.java_ok = runtimes.iter().any(|runtime| runtime.major >= 21);
        self.java_label = runtimes
            .iter()
            .find(|runtime| runtime.major >= 21)
            .map(|runtime| runtime.label());
        self.javas = runtimes;
        self.tuning_cache.clear();
    }

    /// 这个版本单独的启动参数；没设置过时全部跟随全局。
    fn version_cfg(&self, row_id: &str) -> VersionLaunch {
        self.launch_cfgs.get(row_id).cloned().unwrap_or_default()
    }

    /// 全局设置叠上这个版本自己的设置。
    fn tuning_settings(&self, cfg: &VersionLaunch) -> tuning::Settings {
        tuning::Settings {
            memory_mb: cfg.memory.map_or(self.memory, MemoryPick::memory_mb),
            min_memory_mb: None,
            gc: cfg.gc.unwrap_or(self.gc),
            extra: tuning::split_args(cfg.jvm_args.as_deref().unwrap_or(&self.jvm_args)),
            ..Default::default()
        }
    }

    /// 指定路径的 Java：先在检测到的里找，找不到再读一次它的版本。
    fn java_at(&mut self, path: &std::path::Path) -> Option<java::JavaRuntime> {
        let same = |runtime: &java::JavaRuntime| {
            runtime.path == path
                || std::fs::canonicalize(&runtime.path).ok() == std::fs::canonicalize(path).ok()
        };
        if let Some(found) = self.javas.iter().chain(&self.extra_javas).find(|r| same(r)) {
            return Some(found.clone());
        }
        let runtime = java::probe(path).ok()?;
        self.extra_javas.push(runtime.clone());
        Some(runtime)
    }

    /// 这个版本启动时会用的堆和 GC 参数，和 `launch` 的算法相同；只在缓存失效时读盘。
    fn tuning_for(&mut self, row: &Row) -> Option<tuning::Plan> {
        let cfg = self.version_cfg(&row.id);
        self.plan_for(row, &cfg).map(|(plan, _)| plan)
    }

    /// 缓存按「版本 + 指定的 Java」区分：启动参数窗口里换 Java 时不会串。
    fn tuning_key(row_id: &str, cfg: &VersionLaunch) -> String {
        format!("{row_id}\u{1f}{:?}", cfg.java)
    }

    /// 按给定的启动参数（可以是还没保存的草稿）算出会用的 JVM 参数和 Java。
    fn plan_for(
        &mut self,
        row: &Row,
        cfg: &VersionLaunch,
    ) -> Option<(tuning::Plan, java::JavaRuntime)> {
        let key = Self::tuning_key(&row.id, cfg);
        if !self.tuning_cache.contains_key(&key) {
            let game_dir = row
                .game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf());
            let chosen = cfg.java.as_deref().and_then(|path| self.java_at(path));
            let entry = if cfg.java.is_some() && chosen.is_none() {
                None
            } else {
                tuning::workload(
                    &self.root,
                    &row.profile,
                    &game_dir,
                    &self.javas,
                    chosen.as_ref(),
                )
                .ok()
                .map(|(work, java)| {
                    let learned = learned::load(&self.root, &row.profile, &game_dir);
                    (work, java, learned)
                })
            };
            self.machine = tuning::Machine::detect();
            self.tuning_cache.insert(key.clone(), entry);
        }
        let (work, java, learned) = self.tuning_cache.get(&key)?.as_ref()?;
        let settings = tuning::Settings {
            bench: self
                .bench_store
                .as_ref()
                .and_then(|store| store.verdict(java)),
            learned: learned.clone(),
            ..self.tuning_settings(cfg)
        };
        Some((tuning::plan(&self.machine, work, &settings), java.clone()))
    }

    /// 缓存里这个版本当前设置对应的那一条。
    fn tuning_entry(&self, row_id: &str) -> Option<&TuningEntry> {
        let key = Self::tuning_key(row_id, &self.version_cfg(row_id));
        self.tuning_cache.get(&key)?.as_ref()
    }

    /// 启动台下方的截图墙：游戏真正写截图的目录（和启动时的游戏目录一致）。
    fn screenshot_strip(&mut self, ui: &mut egui::Ui, row: &Row, width: f32) {
        let dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.dir().to_path_buf())
            .join("screenshots");
        if self.shots.as_ref().is_none_or(|shots| shots.dir() != dir) {
            self.shots = Some(shots::Shots::new(
                ui.ctx(),
                dir.clone(),
                self.root.cache().join("thumbs"),
            ));
        }
        let Some(shots) = &mut self.shots else {
            return;
        };
        if shots.strip(ui, width) {
            let _ = std::fs::create_dir_all(&dir);
            open_folder(&dir);
        }
    }

    /// 例如「内存 5 GB（自动）」。
    fn memory_label(&mut self, row: &Row) -> String {
        match self.tuning_for(row) {
            Some(plan) if plan.auto_memory => format!("内存 {}（自动）", tuning::gb(plan.heap_mb)),
            Some(plan) => format!("内存 {}", tuning::gb(plan.heap_mb)),
            None => match self.memory {
                Some(mb) => format!("内存 {}", tuning::gb(mb)),
                None => "内存自动".into(),
            },
        }
    }

    fn start_bench(&mut self) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.busy = Some(Task::Bench);
        self.cancel_requested = false;
        cancel::reset();
        self.progress = Some(Progress::stage("准备测试"));
        self.progress_label = "这台电脑".into();
        self.log.push("测一下这台电脑".into());
        let root = self.root.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let progress = tx.clone();
            let sink = from_fn(move |p| {
                let _ = progress.send(Msg::Progress(p));
            });
            let msg = match bench::run(&root, &sink) {
                Ok(store) => Msg::BenchDone(Ok(store)),
                Err(Error::Cancelled) => Msg::BenchCancelled,
                Err(e) => Msg::BenchDone(Err(e.to_string())),
            };
            let _ = tx.send(msg);
        });
    }

    fn finish_bench(&mut self) {
        self.busy = None;
        self.rx = None;
        self.progress = None;
        self.cancel_requested = false;
        cancel::reset();
        self.tuning_cache.clear();
    }

    /// 开始测试前说清楚会发生什么。
    fn bench_overlay(&mut self, ctx: &egui::Context) {
        if !self.bench_confirm {
            return;
        }
        let (picked, _) = bench::candidates(&self.javas);
        let can_run = !picked.is_empty();
        let mut start = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("bench_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(
                    egui::RichText::new("测一下这台电脑")
                        .size(24.0)
                        .family(display_family()),
                );
                ui.label(
                    egui::RichText::new(if can_run {
                        format!(
                            "用 {} 比较 G1 和 ZGC，结论所有版本共用",
                            picked
                                .iter()
                                .map(|java| format!("Java {}", java.major))
                                .collect::<Vec<_>>()
                                .join("、")
                        )
                    } else {
                        "这台电脑没有 64 位的 Java 21 或更新版本：只能用 G1，不需要测试".into()
                    })
                    .size(13.0)
                    .color(if can_run { t::sub() } else { t::warn() }),
                );
                ui.add_space(14.0);
                let minutes = (picked.len() as f32 * 1.4).ceil().max(1.0) as u32;
                for line in [
                    "不开游戏，也不碰任何版本：后台跑一个模拟游戏负载的小程序".to_string(),
                    "负载照游戏来：常驻数据、区块缓存、生成线程、服务端和不封顶的渲染".to_string(),
                    "比平均帧率、1% 低帧、能看出来的顿和 CPU，分常规和大型整合包两档".to_string(),
                    format!("大约 {minutes} 分钟，期间电脑会比较忙，可以随时停止"),
                    "内存大小不在这里测：正常玩一局，启动器会从游戏里学".to_string(),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("·").size(13.0).color(t::dim()));
                        ui.label(egui::RichText::new(line).size(13.0));
                    });
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if motion::primary_sized(ui, "开始测试", can_run, 120.0, 34.0, 13.0) {
                        start = true;
                    }
                    ui.add_space(8.0);
                    if motion::secondary_sized(ui, "取消", true, 72.0, 34.0, 13.0).clicked() {
                        close = true;
                    }
                });
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if start {
            self.bench_confirm = false;
            self.start_bench();
        } else if close {
            self.bench_confirm = false;
        }
    }

    /// 设置里的性能卡片：内存、回收器，以及当前版本会用的参数和计算过程。
    fn tuning_card(&mut self, ui: &mut egui::Ui) {
        let row = self.selected_row();
        let plan = row.as_ref().and_then(|row| self.tuning_for(row));
        egui::Frame::new()
            .fill(t::bg())
            .stroke(egui::Stroke::new(1.0, t::hairline()))
            .corner_radius(motion::R)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(442.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("内存").size(13.0).color(t::sub()));
                    let auto_mb = plan.as_ref().map_or(4096, |plan| {
                        if plan.auto_memory { plan.heap_mb } else { 4096 }
                    });
                    memory_stepper(ui, &mut self.memory, auto_mb);
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("回收器").size(13.0).color(t::sub()));
                    for gc in tuning::GcChoice::ALL {
                        if chip(ui, gc.label(), self.gc == gc).clicked() {
                            self.gc = gc;
                        }
                    }
                });
                let (Some(row), Some(plan)) = (row, plan) else {
                    return;
                };
                let entry = self
                    .tuning_entry(&row.id)
                    .map(|(work, java, learned)| (work.clone(), java.clone(), learned.clone()));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(plan.headline()).size(14.0));
                    // 版本名占剩下的地方，太长就省略，不和右边的链接重叠。
                    let room = (ui.available_width() - 72.0).max(40.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(room, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&row.label).size(13.0).color(t::dim()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&row.label);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.show_tuning {
                            "收起"
                        } else {
                            "查看计算"
                        };
                        if action_link(ui, label).clicked() {
                            self.show_tuning = !self.show_tuning;
                        }
                    });
                });
                ui.label(egui::RichText::new(&plan.basis).size(13.0).color(t::dim()));
                // 这里是全局默认；这个版本自己改过的项以启动参数为准，说清楚免得以为没生效。
                let cfg = self.version_cfg(&row.id);
                if !cfg.is_default() {
                    let mut own = Vec::new();
                    if cfg.java.is_some() {
                        own.push("Java");
                    }
                    if cfg.memory.is_some() {
                        own.push("内存");
                    }
                    if cfg.gc.is_some() {
                        own.push("回收器");
                    }
                    if cfg.jvm_args.is_some() {
                        own.push("JVM 参数");
                    }
                    if cfg.window.is_some() {
                        own.push("窗口");
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "这个版本单独设置了{}，以启动参数为准",
                                own.join("、")
                            ))
                            .size(13.0)
                            .color(t::accent()),
                        );
                        if action_link(ui, "去看看").clicked() {
                            self.show_settings = false;
                            self.open_launch_edit(&row);
                        }
                    });
                }
                for warning in &plan.warnings {
                    ui.label(egui::RichText::new(warning).size(13.0).color(t::warn()));
                }
                // 内存从哪来：玩过就按实测，没玩过提示一句。
                if plan.auto_memory
                    && entry
                        .as_ref()
                        .is_none_or(|(_, _, learned)| learned.as_ref().is_none_or(|l| l.live_mb.is_none()))
                {
                    ui.label(
                        egui::RichText::new("内存现在按公式估算；正常玩一局后改按游戏里的实测")
                            .size(13.0)
                            .color(t::dim()),
                    );
                }
                // 回收器从哪来：这台电脑测过没有。
                let java = entry.as_ref().map(|(_, java, _)| java.clone());
                let (text, color, action) = self.bench_status(java.as_ref());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).size(13.0).color(color))
                            .truncate(),
                    );
                    if let Some(action) = action {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_link(ui, action).clicked() {
                                self.bench_confirm = true;
                            }
                        });
                    }
                });
                if !self.show_tuning {
                    if !self.jvm_args.trim().is_empty() {
                        ui.label(
                            egui::RichText::new("另有自定义 JVM 参数")
                                .size(13.0)
                                .color(t::dim()),
                        );
                    }
                    return;
                }
                ui.add_space(8.0);
                for step in &plan.steps {
                    ui.label(egui::RichText::new(step).size(13.0).color(t::sub()));
                }
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(plan.args().join(" "))
                            .size(13.0)
                            .monospace()
                            .color(t::sub()),
                    )
                    .wrap(),
                );
                if let Some(store) = &self.bench_store {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "这台电脑的测试（{}）",
                            bench::age(store.when)
                        ))
                        .size(13.0)
                        .color(t::sub()),
                    );
                    for result in &store.javas {
                        for tier in &result.tiers {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Java {} · {}档 → {}",
                                    result.major,
                                    tier.label,
                                    tier.collector.label()
                                ))
                                .size(13.0)
                                .color(t::sub()),
                            );
                            for run in [&tier.g1, &tier.zgc] {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "    {} {}：平均 {:.0} 帧 · 1% 低 {:.0} 帧 · 每分钟顿 {:.0} 次{}",
                                        run.collector.label(),
                                        tuning::gb(run.heap_mb),
                                        run.fps,
                                        run.low_fps,
                                        run.hitches_per_min,
                                        run.cpu_cores
                                            .map(|c| format!(" · CPU {c:.1} 核"))
                                            .unwrap_or_default()
                                    ))
                                    .size(13.0)
                                    .color(t::dim()),
                                );
                            }
                        }
                        for note in &result.notes {
                            ui.label(egui::RichText::new(note).size(13.0).color(t::dim()));
                        }
                    }
                    for note in &store.skipped {
                        ui.label(
                            egui::RichText::new(format!("没测：{note}"))
                                .size(13.0)
                                .color(t::dim()),
                        );
                    }
                }
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("额外 JVM 参数")
                        .size(13.0)
                        .color(t::sub()),
                );
                text_field(
                    ui,
                    &mut self.jvm_args,
                    "和自动参数冲突时以这里为准",
                    ui.available_width(),
                );
            });
    }

    /// 回收器那一行：测过没有、结论是什么、能不能再测。
    fn bench_status(
        &self,
        java: Option<&java::JavaRuntime>,
    ) -> (String, egui::Color32, Option<&'static str>) {
        let testable = !bench::candidates(&self.javas).0.is_empty();
        let Some(store) = &self.bench_store else {
            return if testable {
                (
                    "回收器按经验选；测一下这台电脑更准，约 1–2 分钟".into(),
                    t::dim(),
                    Some("测一下"),
                )
            } else {
                ("没有 Java 21+，回收器只能用 G1".into(), t::dim(), None)
            };
        };
        if !store.fresh() {
            return (
                "上次测试后电脑的核数或内存变了，结论不再使用".into(),
                t::warn(),
                testable.then_some("重新测试"),
            );
        }
        let age = bench::age(store.when);
        match java {
            Some(java) if java.major < 21 => (
                format!("这个版本用 Java {}，只用 G1，不需要测试", java.major),
                t::dim(),
                testable.then_some("重新测试"),
            ),
            Some(java) if java.is_graal() => (
                "这个版本用 GraalVM，只用 G1，不需要测试".into(),
                t::dim(),
                testable.then_some("重新测试"),
            ),
            Some(java) => match store.find(java) {
                Some(result) => {
                    let tiers = result
                        .tiers
                        .iter()
                        .map(|tier| format!("{}档 {}", tier.label, tier.collector.label()))
                        .collect::<Vec<_>>()
                        .join("、");
                    (
                        format!("{age}测过这台电脑：Java {} {tiers}", result.major),
                        t::dim(),
                        Some("重新测试"),
                    )
                }
                None => (
                    format!("{age}的测试里没有 Java {}，回收器按经验选", java.major),
                    t::dim(),
                    Some("重新测试"),
                ),
            },
            None => (format!("{age}测过这台电脑"), t::dim(), Some("重新测试")),
        }
    }

    fn refresh(&mut self) {
        self.home_cache = None;
        self.home_verified = None;
        self.tuning_cache.clear();
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
        let kind = if self.login_error.is_some() {
            3
        } else if self.login_url.is_some() {
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
        let raw = if self.reduce_motion {
            1.0
        } else {
            (((now - self.modal_started) / 0.15) as f32).clamp(0.0, 1.0)
        };
        let open_e = motion::ease_out_cubic(raw);
        if kind != 0 && raw < 1.0 {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let backdrop = (178.0 * open_e) as u8;
        let veil = t::veil(backdrop as f32 / 178.0);
        let frame_fill = egui::Color32::from_rgba_unmultiplied(
            t::surface().r(),
            t::surface().g(),
            t::surface().b(),
            (255.0 * open_e) as u8,
        );
        let frame_stroke = egui::Color32::from_rgba_unmultiplied(
            t::hairline().r(),
            t::hairline().g(),
            t::hairline().b(),
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
                        .stroke(egui::Stroke::new(2.0, frame_stroke))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(26, 22)),
                )
                .show(ctx, |ui| {
                    ui.multiply_opacity(0.35 + 0.65 * open_e);
                    ui.add_space((1.0 - open_e) * 6.0);
                    ui.set_width(368.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("在浏览器中完成登录")
                                .size(24.0)
                                .family(display_family())
                                .color(t::ink()),
                        );
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "登录成功后会自动回到启动器，不需要手动复制代码。",
                                )
                                .size(13.0)
                                .color(t::sub()),
                            )
                            .wrap(),
                        );
                        ui.add_space(14.0);
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 11))
                            .show(ui, |ui| {
                                ui.set_width(310.0);
                                ui.horizontal(|ui| {
                                    busy_spinner(ui);
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new("等待微软授权…")
                                            .size(13.0)
                                            .color(t::sub()),
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
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
            if modal.backdrop_response.clicked() || escape_pressed(ctx) {
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
                self.login_error = None;
                self.notice = Some("已取消登录".into());
            }
        } else if let Some(message) = self.login_error.clone() {
            let mut close = false;
            let mut retry = false;
            let modal = egui::Modal::new(egui::Id::unique("login_error_modal"))
                .backdrop_color(veil)
                .frame(
                    egui::Frame::new()
                        .fill(frame_fill)
                        .stroke(egui::Stroke::new(2.0, frame_stroke))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(26, 22)),
                )
                .show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.label(
                        egui::RichText::new("登录没有完成")
                            .size(24.0)
                            .family(display_family())
                            .color(t::ink()),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "浏览器里的授权没有回到启动器。可以重试，或稍后再登录。",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        )
                        .wrap(),
                    );
                    ui.add_space(12.0);
                    egui::Frame::new()
                        .fill(t::tint(t::warn(), 0.11))
                        .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.41)))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(332.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&message).size(13.0).color(t::warn()),
                                )
                                .wrap(),
                            );
                        });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if motion::primary_sized(ui, "重新登录", true, 104.0, 32.0, 11.5) {
                                retry = true;
                            }
                            ui.add_space(6.0);
                            if motion::secondary_sized(ui, "关闭", true, 72.0, 32.0, 11.0).clicked()
                            {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new("原始错误已写入「设置 › 诊断 › 日志」")
                            .size(13.0)
                            .color(t::sub()),
                    );
                });
            if modal.backdrop_response.clicked() || escape_pressed(ctx) {
                close = true;
            }
            if retry {
                self.login_error = None;
                self.start_login();
            } else if close {
                self.login_error = None;
                self.show_account = false;
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
                        .stroke(egui::Stroke::new(2.0, frame_stroke))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(24, 20)),
                )
                .show(ctx, |ui| {
                    ui.multiply_opacity(0.35 + 0.65 * open_e);
                    ui.add_space((1.0 - open_e) * 6.0);
                    ui.set_width(360.0);
                    ui.horizontal(|ui| {
                        avatar_sized(ui, &name, 46.0);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&name)
                                    .size(24.0)
                                    .family(display_family()),
                            );
                            ui.horizontal(|ui| {
                                status_dot(ui, true);
                                ui.label(
                                    egui::RichText::new("Microsoft 账户 · 已登录")
                                        .size(13.0)
                                        .color(t::sub()),
                                );
                            });
                        });
                    });
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("UUID").size(13.0).color(t::sub()));
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(short_text(&uuid, 32))
                                    .size(13.0)
                                    .color(t::ink()),
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
                            .size(13.0)
                            .color(t::sub()),
                    );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("退出登录")
                                        .size(13.0)
                                        .color(t::danger()),
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
            if modal.backdrop_response.clicked() || escape_pressed(ctx) {
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
        let mut recheck_java = false;
        let mut open_log = false;
        let veil = t::veil(1.0);
        let modal = egui::Modal::new(egui::Id::unique("settings_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(470.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("设置")
                                .size(24.0)
                                .family(display_family()),
                        );
                        ui.label(
                            egui::RichText::new("账户、内存、Java 与本地目录")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
                ui.add_space(14.0);
                let height = ctx.content_rect().height();
                let body_max = (height - 190.0).max(280.0);
                egui::ScrollArea::vertical()
                    .max_height(body_max)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
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
                                                    .size(13.0)
                                                    .color(t::ok()),
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
                                                    .size(13.0)
                                                    .color(t::sub()),
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
                                            if motion::secondary_sized(
                                                ui, label, true, 112.0, 30.0, 10.5,
                                            )
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
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("玩家名").size(13.0).color(t::sub()),
                                    );
                                    text_field(ui, &mut self.username, "", 150.0);
                                });
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    status_dot(ui, self.java_ok);
                                    let label = self.java_label.clone().unwrap_or_else(|| {
                                        if self.java_ok {
                                            "Java 21+ 已检测".to_string()
                                        } else {
                                            "未检测到 Java 21+".to_string()
                                        }
                                    });
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(short_text(&label, 34))
                                                .size(13.0)
                                                .color(if self.java_ok {
                                                    t::sub()
                                                } else {
                                                    t::warn()
                                                }),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&label);
                                    if action_link(ui, "重新检测").clicked() {
                                        recheck_java = true;
                                    }
                                    if !self.java_ok && action_link(ui, "下载 Java 21").clicked()
                                    {
                                        open_url(
                                            "https://adoptium.net/temurin/releases/?version=21",
                                        );
                                    }
                                });
                            });
                        ui.add_space(12.0);

                        self.tuning_card(ui);
                        ui.add_space(12.0);

                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    if path_row(ui, "游戏目录", &game_dir) {
                                        open_dir = Some(game_dir.clone());
                                    }
                                });
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if path_row(ui, "数据目录", self.root.dir()) {
                                        open_dir = Some(self.root.dir().to_path_buf());
                                    }
                                });
                            });
                        ui.add_space(12.0);

                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("外观").size(13.0).color(t::sub()),
                                    );
                                    ui.add_space(8.0);
                                    for pref in t::Preference::ALL {
                                        if chip(ui, pref.label(), self.theme_pref == pref).clicked()
                                        {
                                            self.theme_pref = pref;
                                            pref.apply(ui.ctx());
                                            self.save_ui_config();
                                        }
                                    }
                                });
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if toggle_switch(ui, self.reduce_motion).clicked() {
                                        self.reduce_motion = !self.reduce_motion;
                                        motion::set_reduced(self.reduce_motion);
                                    }
                                    ui.label(egui::RichText::new("减少动效").size(13.0).color(
                                        if self.reduce_motion {
                                            t::ink()
                                        } else {
                                            t::sub()
                                        },
                                    ));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("关闭过渡和持续动画")
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                        },
                                    );
                                });
                            });
                        ui.add_space(12.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("诊断")
                                    .size(13.0)
                                    .family(bold_family())
                                    .color(t::sub()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if action_link(ui, "刷新版本列表").clicked() {
                                        refresh = true;
                                    }
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(match &self.last_launch {
                                    Some(last) => format!(
                                        "最近一次启动：{}",
                                        short_text(&last.label, 24)
                                    ),
                                    None => format!("启动器记录 {} 条", self.log.len()),
                                })
                                .size(13.0)
                                .color(t::sub()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if action_link(ui, "查看日志").clicked() {
                                        open_log = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(
                                "快捷键：Ctrl+1–4 切换页面 · Ctrl+F 搜索 · Ctrl+L 日志 · Ctrl+, 设置 · Esc 关闭弹窗",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                ui.add_space(8.0);
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
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
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
        if recheck_java {
            self.refresh_java();
        }
        if close || self.bench_confirm || open_log {
            self.save_ui_config();
            self.show_settings = false;
        }
        if open_log {
            self.open_log();
        }
    }

    fn save_ui_config(&self) {
        let value = serde_json::json!({
            "username": self.username,
            "memory": self.memory,
            "gc": self.gc.key(),
            "jvm_args": self.jvm_args.trim(),
            "reduce_motion": self.reduce_motion,
            "theme": self.theme_pref.key(),
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
                if let Some(note) = &self.rollback_notice {
                    ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
                    ui.add_space(6.0);
                }
                if let Some(err) = &self.rollback_error {
                    ui.label(egui::RichText::new(err).size(13.0).color(t::danger()));
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if self.rollback_busy {
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
                        if self.rollback_points.is_empty() {
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
                        for point in &self.rollback_points {
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
                                                if self.rollback_confirm.as_deref()
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

    fn start_login(&mut self) {
        if self.login_busy {
            return;
        }
        cancel::reset();
        self.cancel_requested = false;
        self.login_busy = true;
        self.login_url = None;
        self.login_error = None;
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
            self.error = Some("这个版本缺少文件，先补全再启动".into());
            return;
        }
        let cfg = self.version_cfg(&row.id);
        let required = launch::required_java_for(&self.root, &row.profile).unwrap_or(8);
        match &cfg.java {
            Some(path) => match self.java_at(path) {
                Some(java) if java.major < required => {
                    self.error = Some(format!(
                        "这个版本需要 Java {required}，启动参数里指定的是 Java {}；在启动参数里换一个",
                        java.major
                    ));
                    return;
                }
                Some(_) => {}
                None => {
                    self.error = Some(format!(
                        "启动参数里指定的 Java 不见了：{}；在启动参数里换一个",
                        path.display()
                    ));
                    return;
                }
            },
            None => {
                if !self.javas.iter().any(|java| java.major >= required) {
                    self.refresh_java();
                }
                if !self.javas.iter().any(|java| java.major >= required) {
                    self.error = Some(format!(
                        "这个版本需要 Java {required} 或更新，没有找到；可以在设置里打开下载页"
                    ));
                    return;
                }
            }
        }
        self.error = None;
        self.notice = None;
        self.busy = Some(Task::Launch);
        self.launch_started = Some(Instant::now());
        self.running_row = Some(row.id.clone());
        self.log.push(format!("启动 {}", row.id));
        self.last_launch = Some(logview::LastLaunch {
            label: row.label.clone(),
            log: launch::log_path(&self.root, &row.profile),
            game_dir: row
                .game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf()),
            started: std::time::SystemTime::now(),
            outcome: logview::Outcome::Running,
        });
        self.tuning_cache
            .retain(|key, _| !key.starts_with(&format!("{}\u{1f}", row.id)));
        if let Some(plan) = self.tuning_for(&row) {
            self.log
                .push(format!("JVM {}，{}", plan.headline(), plan.basis));
            self.log
                .extend(plan.warnings.iter().map(|w| format!("注意：{w}")));
        }
        let root = self.root.clone();
        let username = self.username.clone();
        let tuning = self.tuning_settings(&cfg);
        let [width, height] = cfg.window();
        let java_path = cfg.java.clone();
        let account = self.account.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let mut opts = LaunchOptions {
                username,
                memory_mb: tuning.memory_mb,
                gc: tuning.gc,
                jvm_args: tuning.extra,
                game_dir: row.game_dir.clone(),
                java: java_path,
                width,
                height,
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
                Ok(0) => Msg::LaunchDone(0, None),
                Ok(code) => {
                    // 在后台线程里读日志找原因，界面不用等。
                    let reason = std::fs::read(launch::log_path(&root, &row.profile))
                        .ok()
                        .and_then(|bytes| {
                            let tail = &bytes[bytes.len().saturating_sub(2 << 20)..];
                            launch::diagnose(&String::from_utf8_lossy(tail))
                        });
                    Msg::LaunchDone(code, reason)
                }
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
        self.mods_installed_dir = None;
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

    /// 拖进来的内容装到哪：在内容页就是上面选的目标，其他页面是当前版本。
    fn drop_target(&self) -> Option<(Row, PathBuf)> {
        if self.show_mods {
            return self.mods_row.clone().zip(self.mods_game_dir.clone());
        }
        let row = self.selected_row()?;
        let dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        Some((row, dir))
    }

    /// 把拖进来的文件（或文件夹里的文件）分成内容、整合包和认不出的。
    fn plan_drop(&self, paths: &[PathBuf]) -> DropPlan {
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                let mut inside: Vec<PathBuf> = std::fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|p| {
                        p.is_file() && installed::classify(p) != installed::Dropped::Unknown
                    })
                    .collect();
                inside.sort();
                files.extend(inside);
            } else {
                files.push(path.clone());
            }
        }
        // 在资源包、光影页拖进不认识的 zip，就按当前页的类别装。
        let page_kind =
            (self.show_mods && self.mods_kind != mods::ContentKind::Mod).then_some(self.mods_kind);
        let mut plan = DropPlan {
            target: self.drop_target(),
            ..Default::default()
        };
        for file in files {
            match installed::classify(&file) {
                installed::Dropped::Content(kind) => plan.items.push((file, kind)),
                installed::Dropped::Modpack => plan.packs.push(file),
                installed::Dropped::Archive => match page_kind {
                    Some(kind) => plan.items.push((file, kind)),
                    None => plan.unknown.push(file),
                },
                installed::Dropped::Unknown => plan.unknown.push(file),
            }
        }
        plan
    }

    /// 悬停时的一句话：会装进哪、装几个；不能装时说为什么。
    fn describe_drop(&self, plan: &DropPlan) -> (String, bool) {
        if plan.items.is_empty() {
            return match (plan.packs.len(), plan.unknown.len()) {
                (1, 0) => ("松开导入整合包，自动装好加载器".into(), self.busy.is_none()),
                (0, _) => (
                    "认不出这些文件：支持 Mod .jar、资源包和光影 .zip、整合包 .mrpack".into(),
                    false,
                ),
                _ => ("整合包一次拖一个".into(), false),
            };
        }
        let Some((row, _)) = &plan.target else {
            return ("先选一个版本：内容只会装进当前版本".into(), false);
        };
        if let Some(reason) = self.write_block(Some(&row.id)) {
            return (reason.into(), false);
        }
        if plan
            .items
            .iter()
            .any(|(_, kind)| *kind == mods::ContentKind::Mod)
            && mod_loader(row.loader).is_none()
        {
            return (
                format!("「{}」是原版，不能装 Mod", short_text(&row.label, 20)),
                false,
            );
        }
        let mut extra = String::new();
        if !plan.packs.is_empty() || !plan.unknown.is_empty() {
            extra = format!(
                "（另有 {} 个不是内容，会跳过）",
                plan.packs.len() + plan.unknown.len()
            );
        }
        (
            format!(
                "松开导入 {} → {}{extra}",
                count_kinds(plan.items.iter().map(|(_, kind)| *kind)),
                short_text(&row.label, 24)
            ),
            true,
        )
    }

    /// 导入一批拖进来或选中的文件：整批只留一个恢复点，已经装过的跳过。
    fn import_dropped(&mut self, plan: DropPlan) {
        let on_content = self.show_mods;
        let mut imported: Vec<mods::ContentKind> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        let mut marked = false;
        if !plan.items.is_empty() {
            let (reason, ok) = self.describe_drop(&DropPlan {
                target: plan.target.clone(),
                items: plan.items.clone(),
                ..Default::default()
            });
            match (&plan.target, ok) {
                (Some((row, game_dir)), true) => {
                    let fresh: Vec<&(PathBuf, mods::ContentKind)> = plan
                        .items
                        .iter()
                        .filter(|(path, kind)| {
                            let file = path
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let known = installed::contains(game_dir, *kind, &file);
                            if known {
                                skipped.push(file);
                            }
                            !known
                        })
                        .collect();
                    if !fresh.is_empty() {
                        if row.instance.is_none()
                            && !self.root.isolated_marker(&row.profile).exists()
                        {
                            let _ = std::fs::create_dir_all(game_dir);
                            match std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                                Ok(()) => marked = true,
                                Err(e) => problems.push(format!("启用版本隔离失败: {e}")),
                            }
                        }
                        let loader = loader_state_files(&self.root, row, game_dir);
                        if problems.is_empty() {
                            if let Err(e) = rollback::backup_extra(game_dir, "导入内容前", &loader)
                            {
                                problems.push(format!("无法创建恢复点: {e}"));
                            } else {
                                for (path, kind) in fresh {
                                    match installed::import_local(game_dir, *kind, path) {
                                        Ok(entry) => {
                                            self.mods_recent.insert(entry.file.clone());
                                            names.push(entry.name);
                                            imported.push(*kind);
                                        }
                                        Err(e) => problems.push(e.to_string()),
                                    }
                                }
                            }
                        }
                    }
                }
                _ => problems.push(reason),
            }
        }
        let lone_pack = plan.items.is_empty() && plan.unknown.is_empty() && plan.packs.len() == 1;
        if !plan.unknown.is_empty() {
            let shown = plan
                .unknown
                .iter()
                .take(2)
                .filter_map(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
                .collect::<Vec<_>>()
                .join("、");
            problems.push(format!(
                "认不出 {shown}{}：支持 Mod .jar、资源包和光影 .zip、整合包 .mrpack",
                if plan.unknown.len() > 2 { " 等" } else { "" }
            ));
        }
        if !plan.packs.is_empty() && !lone_pack {
            problems.push("整合包要单独拖进来，一次一个".into());
        }

        let mut notice = None;
        if !imported.is_empty() {
            let mut text = format!(
                "已导入 {}：{}",
                count_kinds(imported.iter().copied()),
                short_text(&names.join("、"), 60)
            );
            if !skipped.is_empty() {
                text.push_str(&format!("；{} 个已经装过，跳过", skipped.len()));
            }
            notice = Some(text);
        } else if !skipped.is_empty() {
            notice = Some(format!("{} 个文件已经装过，没有重复导入", skipped.len()));
        }
        let error = (!problems.is_empty()).then(|| problems.join("；"));
        if on_content {
            self.mods_notice = notice;
            self.mods_error = error;
        } else {
            self.notice = notice;
            self.error = error;
        }
        if lone_pack {
            if self.busy.is_none() {
                self.start_modpack(plan.packs[0].clone());
            } else {
                self.error = Some(self.busy_reason().into());
            }
            return;
        }
        if imported.is_empty() {
            return;
        }
        if marked {
            self.refresh();
        }
        if on_content {
            // 新导入的要看得见：清掉筛选；全是另一类时切到那一类。
            self.mods_status = StatusFilter::All;
            self.mods_filter.clear();
            if let Some(&kind) = imported.first()
                && imported.iter().all(|k| *k == kind)
                && kind != self.mods_kind
            {
                self.mods_kind = kind;
            }
            self.start_mods_scan();
        } else {
            self.home_cache = None;
            self.home_verified = None;
            self.tuning_cache.clear();
        }
    }

    /// 文件管理器里改过内容目录、回到窗口时自动重扫。
    fn rescan_if_changed(&mut self) {
        if !(self.show_mods && self.mods_show_installed) || self.mods_scan_rx.is_some() {
            return;
        }
        let Some(game_dir) = &self.mods_game_dir else {
            return;
        };
        if content_signature(game_dir) != self.mods_dir_sig {
            self.mods_notice = None;
            self.start_mods_scan();
        }
    }

    /// 拖着文件经过窗口时，盖一层说明会装到哪。安装页有自己的拖放区，不盖。
    fn drop_overlay(&mut self, ui: &mut egui::Ui) {
        let hovered: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        let dragging = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        if !dragging || self.show_install {
            self.drop_preview = None;
            return;
        }
        let stale = self
            .drop_preview
            .as_ref()
            .is_none_or(|(paths, _, _)| *paths != hovered);
        if stale {
            let (text, ok) = if hovered.is_empty() {
                ("松开导入".to_string(), true)
            } else {
                self.describe_drop(&self.plan_drop(&hovered))
            };
            self.drop_preview = Some((hovered, text, ok));
        }
        let Some((_, text, ok)) = &self.drop_preview else {
            return;
        };
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, t::tint(t::bg(), 0.88));
        let zone = rect.shrink(24.0);
        let color = if *ok { t::accent() } else { t::warn() };
        painter.rect_stroke(
            zone,
            motion::R,
            egui::Stroke::new(1.5, t::tint(color, 0.7)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            zone.center() - egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            if *ok {
                "松开导入"
            } else {
                "这里不能导入"
            },
            egui::FontId::new(22.0, display_family()),
            t::ink(),
        );
        painter.text(
            zone.center() + egui::vec2(0.0, 18.0),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(14.0),
            color,
        );
    }

    fn start_mods_scan(&mut self) {
        self.home_cache = None;
        self.home_verified = None;
        self.tuning_cache.clear();
        self.version_cache = None;
        let Some(game_dir) = self.mods_game_dir.clone() else {
            return;
        };
        // 先把上次的索引摆出来（读一个 JSON，几毫秒），后台再核对磁盘；
        // 核对只重读大小或修改时间变了的文件，结果回来就替换，所以不会一直显示旧的。
        if self.mods_installed_dir.as_ref() != Some(&game_dir) {
            self.mods_installed = installed::load(&game_dir).entries;
            self.mods_installed_dir = Some(game_dir.clone());
        }
        self.mods_installed_loading = self.mods_installed.is_empty();
        self.mods_error = None;
        self.mods_dir_sig = content_signature(&game_dir);
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
                updates: Vec::new(),
                quick: true,
                update: false,
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
                updates: Vec::new(),
                quick: false,
                update: false,
            },
            request: InstallRequest::Detail,
        });
    }

    /// 更新同样先确认：列出从哪个版本到哪个版本，别让“全部更新”变成盲盒。
    fn review_update(&mut self, candidates: Vec<installed::UpdateCandidate>) {
        if self.busy.is_some() || candidates.is_empty() {
            return;
        }
        let Some(row) = self.mods_row.clone() else {
            self.mods_error = Some("先在「安装到」选一个版本".into());
            return;
        };
        let Some(path) = self.mods_game_dir.clone() else {
            self.mods_error = Some("目标目录无效".into());
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
                kind: self.mods_kind,
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
                    if matches!(
                        self.busy,
                        Some(Task::Install) | Some(Task::Mods) | Some(Task::Bench)
                    ) {
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
                        Err(e) => self.mods_error = Some(content_error(e)),
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
                        Err(e) => self.mods_error = Some(content_error(e)),
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
                        Err(e) => self.mods_error = Some(content_error(e)),
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
                    self.mods_error = Some(content_error(e));
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
                        Err(e) => self.mods_error = Some(content_error(e)),
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
                        Err(e) => self.mods_error = Some(content_error(e)),
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
                        Err(e) => self.mods_error = Some(content_error(e)),
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
                        Err(e) => self.mods_error = Some(content_error(e)),
                    }
                }
                Msg::SavesDone(result) => {
                    self.saves_busy = false;
                    self.home_cache = None;
                    self.home_verified = None;
                    self.tuning_cache.clear();
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
                            self.login_error = None;
                            self.notice = Some("已登录 Microsoft 账户".into());
                        }
                        Err(e) => {
                            self.login_url = None;
                            self.log.push(format!("登录失败: {e}"));
                            self.error = None;
                            self.login_error = Some(friendly_login_error(&e));
                        }
                    }
                }
                Msg::LaunchDone(code, reason) => {
                    if let Some(last) = &mut self.last_launch {
                        last.outcome = logview::Outcome::Exited(code);
                    }
                    self.busy = None;
                    self.running_row = None;
                    // 每玩完一局就给世界留一份快照，比按时间间隔更贴近“坏了能救”。
                    if self.backup_cfg.enabled && !self.saves_busy {
                        self.start_auto_backup();
                    }
                    self.rx = None;
                    self.tuning_cache.clear();
                    if code == 0 {
                        self.notice = Some("游戏已退出".into());
                    } else {
                        self.log.push(format!("游戏异常退出（代码 {code}）"));
                        self.error = Some(match reason {
                            Some(reason) => format!("游戏异常退出（代码 {code}）：{reason}"),
                            None => {
                                format!("游戏异常退出（代码 {code}），日志里标红的是出错的地方")
                            }
                        });
                    }
                }
                Msg::LaunchFailed(e) => {
                    if let Some(last) = &mut self.last_launch {
                        last.outcome = logview::Outcome::Failed(e.clone());
                    }
                    self.busy = None;
                    self.running_row = None;
                    self.rx = None;
                    self.error = Some(format!("启动失败：{e}"));
                    self.log.push(format!("启动失败: {e}"));
                }
                Msg::BenchDone(result) => {
                    self.finish_bench();
                    match result {
                        Ok(store) => {
                            let summary = store
                                .javas
                                .iter()
                                .map(|java| {
                                    let tiers = java
                                        .tiers
                                        .iter()
                                        .map(|tier| {
                                            format!("{} {}", tier.label, tier.collector.label())
                                        })
                                        .collect::<Vec<_>>()
                                        .join("、");
                                    format!("Java {} {tiers}", java.major)
                                })
                                .collect::<Vec<_>>()
                                .join("；");
                            self.log.push(format!("测试完成：{summary}"));
                            for java in &store.javas {
                                for tier in &java.tiers {
                                    self.log.extend(tier.notes.iter().cloned());
                                }
                            }
                            self.notice =
                                Some(format!("测试完成：{summary}。之后「自动」回收器按它选"));
                            self.bench_store = Some(store);
                            self.tuning_cache.clear();
                            self.show_settings = true;
                            self.show_tuning = true;
                        }
                        Err(e) => {
                            self.log.push(format!("测试失败: {e}"));
                            self.error = Some(format!("测试没完成：{e}"));
                        }
                    }
                }
                Msg::BenchCancelled => {
                    self.finish_bench();
                    self.notice = Some("测试已停止，之前的结论没有改动".into());
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
        if self.page_locked() {
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
        if self.page_locked() {
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
        if self.page_locked() {
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
        let width = if wide { 190.0 } else { 64.0 };
        let active = self.nav_active();
        let account_name = self
            .account
            .as_ref()
            .map(|account| account.name.clone())
            .unwrap_or_else(|| self.username.clone());
        let linked = self.account.is_some();
        let busy = self.busy.is_some();
        let login_busy = self.login_busy;
        let enabled = !self.page_locked();
        let mut go: Option<Nav> = None;
        let mut account_clicked = false;
        let mut settings_clicked = false;

        egui::Panel::left("shell_nav")
            .resizable(false)
            .exact_size(width)
            .frame(egui::Frame::new().fill(t::bg()).inner_margin(egui::Margin {
                left: if wide { 12 } else { 10 },
                right: if wide { 12 } else { 10 },
                top: 20,
                bottom: 14,
            }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                if wide {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Cairn")
                                .size(21.0)
                                .family(light_family())
                                .color(t::ink()),
                        );
                    });
                } else {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("C")
                                .size(24.0)
                                .family(light_family())
                                .color(t::ink()),
                        );
                    });
                }
                ui.add_space(20.0);

                for nav in Nav::ALL {
                    if sidebar_item(ui, nav, active == nav, wide, enabled).clicked() {
                        go = Some(nav);
                    }
                }

                // 底部用显式留白定位：嵌套 bottom_up 在侧边栏里会缩成内容高度。
                let playing = matches!(self.busy, Some(Task::Launch));
                let locked = (busy && !playing) || login_busy;
                let footer = if locked {
                    40.0
                } else if playing {
                    112.0
                } else {
                    78.0
                };
                ui.add_space((ui.available_height() - footer).max(10.0));
                if playing {
                    ui.horizontal(|ui| {
                        ui.add_space(if wide { 14.0 } else { 18.0 });
                        status_dot(ui, true);
                        if wide {
                            ui.label(egui::RichText::new("游戏运行中").size(13.0).color(t::sub()));
                        }
                    })
                    .response
                    .on_hover_text("游戏运行中：可以查看各页，正在玩的版本退出后才能修改");
                    ui.add_space(8.0);
                }
                if locked {
                    ui.horizontal(|ui| {
                        if wide {
                            ui.add_space(12.0);
                        }
                        busy_spinner(ui);
                        if wide {
                            ui.label(
                                egui::RichText::new(if login_busy {
                                    "正在登录…"
                                } else {
                                    "任务进行中"
                                })
                                .size(13.0)
                                .color(t::sub()),
                            );
                        }
                    });
                } else {
                    if sidebar_account(ui, &account_name, linked, wide).clicked() {
                        account_clicked = true;
                    }
                    if sidebar_settings(ui, wide, true).clicked() {
                        settings_clicked = true;
                    }
                }
            });

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
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("欢迎使用 Cairn")
                                        .size(36.0)
                                        .family(light_family())
                                        .color(t::ink()),
                                );
                            });
                        });
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("装得干净，坏了能救。")
                                    .size(13.0)
                                    .color(t::ink()),
                            );
                            ui.label(
                                egui::RichText::new("第一次启动，只需要把这三件事准备好。")
                                    .size(13.0)
                                    .color(t::sub()),
                            );
                        });
                        ui.add_space(20.0);
                        egui::Frame::new()
                            .fill(t::surface())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(24, 20))
                            .show(ui, |ui| {
                                ui.set_width(content_w);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("开始之前")
                                            .size(15.0)
                                            .family(display_family()),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("大约 1 分钟")
                                                    .size(13.0)
                                                    .color(t::sub()),
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
                                                    .size(15.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(
                                                    "启动游戏需要 Java 21 或更高；下载版本本身不需要。",
                                                )
                                                .size(13.0)
                                                .color(t::sub()),
                                            );
                                            if let Some(label) = &self.java_label {
                                                ui.add_space(3.0);
                                                ui.label(
                                                    egui::RichText::new(short_text(label, 46))
                                                        .size(13.0)
                                                        .color(t::ok()),
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
                                                                .size(13.0)
                                                                .color(t::ok()),
                                                        );
                                                    });
                                                } else {
                                                    ui.label(
                                                        egui::RichText::new("未找到合适的 Java")
                                                            .size(13.0)
                                                            .color(t::warn()),
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
                                                    .size(15.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(
                                                    "版本、Mod 和存档都在这里，不写系统目录。",
                                                )
                                                .size(13.0)
                                                .color(t::sub()),
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
                                                            .size(13.0)
                                                            .color(t::sub()),
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
                                                .size(15.0)
                                                .family(bold_family()),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "原版 / Fabric / NeoForge 都可以；整合包会顺带装好加载器和 Mod。",
                                            )
                                            .size(13.0)
                                            .color(t::sub()),
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
                                                .size(13.0)
                                                .color(t::sub()),
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
                                .size(13.0)
                                .color(t::sub()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "v{}",
                                            env!("CARGO_PKG_VERSION")
                                        ))
                                        .size(13.0)
                                        .color(t::sub()),
                                    );
                                },
                            );
                        });
                        ui.add_space(24.0);
                    });
                });
            });
        if recheck {
            self.refresh_java();
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
        let game_dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        let mut cache = self.home_cache_for(&row, &game_dir);
        if self.verify_home_content(ui.ctx(), &row) {
            cache = self.home_cache_for(&row, &game_dir);
        }
        let ring_data = self.ring_data(&row, &cache);
        let running = ring_data.state == rings::State::Running;
        let title = row
            .label
            .split(" · ")
            .next()
            .unwrap_or(&row.label)
            .to_string();
        let compact_h = ui.ctx().content_rect().height() < 680.0;
        let hero_h: f32 = if compact_h { 230.0 } else { 290.0 };
        let width = (ui.available_width() - 72.0).clamp(320.0, 1000.0);
        let pad = ((ui.available_width() - width) / 2.0).max(0.0);

        let mut chosen: Option<String> = None;
        let mut primary = false;
        let mut clicked_layer = None;
        let mut open_saves = false;
        let mut open_content = false;
        let mut open_manage = false;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 内容不满一屏时整体略偏上居中，避免都挤在顶部。
                let spare = ui.available_height() - hero_h - 150.0;
                ui.add_space((spare * 0.4).max(if compact_h { 16.0 } else { 34.0 }));
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        let (hero, _) =
                            ui.allocate_exact_size(egui::vec2(width, hero_h), egui::Sense::hover());
                        let ring_w = hero_h * 300.0 / 260.0;
                        let ring_rect = egui::Rect::from_min_size(
                            egui::pos2(hero.right() - ring_w, hero.top()),
                            egui::vec2(ring_w, hero_h),
                        );
                        clicked_layer = rings::show(ui, ring_rect, &ring_data);

                        let left = egui::Rect::from_min_max(
                            hero.left_top(),
                            egui::pos2(ring_rect.left() - 12.0, hero.bottom()),
                        );
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(left)
                                .layout(egui::Layout::top_down(egui::Align::LEFT)),
                        );
                        child.add_space((hero_h - 206.0).max(0.0) / 2.0);
                        let status = match ring_data.state {
                            rings::State::Running => "游戏运行中".to_string(),
                            rings::State::Missing => {
                                format!("缺 {} 个文件", row.health.missing)
                            }
                            _ => "安装完整".to_string(),
                        };
                        let loader = match &row.loader_version {
                            Some(version) => format!("{} {version}", row.loader.name()),
                            None => row.loader.name().to_string(),
                        };
                        child.label(
                            egui::RichText::new(format!("{loader} · {status}"))
                                .size(13.0)
                                .color(if row.health.missing > 0 {
                                    t::warn()
                                } else {
                                    t::sub()
                                }),
                        );
                        // 版本名完整显示：放得下用大字，放不下按宽度缩小，太长才换成两行。
                        let (size, wrap) = fit_title(&child, &title, left.width());
                        let label = egui::Label::new(
                            egui::RichText::new(&title)
                                .size(size)
                                .family(light_family())
                                .color(t::ink()),
                        );
                        child
                            .add(if wrap { label.wrap() } else { label.extend() })
                            .on_hover_text(&row.label);
                        child.add_space(14.0);
                        child.horizontal(|ui| {
                            let (label, glyph) = match ring_data.state {
                                rings::State::Missing => {
                                    ("补全文件", motion::PrimaryGlyph::Download)
                                }
                                rings::State::Running => {
                                    ("查看运行状态", motion::PrimaryGlyph::None)
                                }
                                _ => ("开始游戏", motion::PrimaryGlyph::Play),
                            };
                            if motion::primary_sized_glyph(
                                ui, label, true, 150.0, 40.0, 14.0, glyph,
                            ) {
                                primary = true;
                            }
                            ui.spacing_mut().interact_size.y = 40.0;
                            let mut picked = row.id.clone();
                            egui::ComboBox::from_id_salt("home_version_switch")
                                .selected_text(egui::RichText::new("切换版本").size(14.0))
                                .width(96.0)
                                .height(240.0)
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
                        });
                        child.add_space(12.0);
                        self.launch_pills(&mut child, &row);
                        // 名字换成两行、窄窗口参数折行时，左边可能比预留的高：把多出来的让出来，
                        // 不压到下面的摘要。
                        let overflow = child.min_rect().bottom() - hero.bottom();
                        if overflow > 0.0 {
                            ui.add_space(overflow + 8.0);
                        }

                        ui.add_space(if compact_h { 10.0 } else { 18.0 });
                        if let Some(note) = &self.notice {
                            ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
                            ui.add_space(6.0);
                        }
                        if let Some(err) = self.error.clone() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(err).size(13.0).color(t::danger()));
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("查看日志")
                                                .size(13.0)
                                                .color(t::sub()),
                                        )
                                        .frame(false),
                                    )
                                    .on_hover_text("直接打开这次的游戏输出（Ctrl+L）")
                                    .clicked()
                                {
                                    self.open_log();
                                }
                            });
                            ui.add_space(6.0);
                        }

                        // 三栏摘要：细线分隔，整栏可点。
                        let top = ui.cursor().top();
                        ui.painter().line_segment(
                            [egui::pos2(hero.left(), top), egui::pos2(hero.right(), top)],
                            egui::Stroke::new(1.0, t::hairline()),
                        );
                        let cell_w = width / 3.0;
                        let (cells, _) =
                            ui.allocate_exact_size(egui::vec2(width, 84.0), egui::Sense::hover());
                        let world = match &cache.latest_world {
                            Some(world) => {
                                format!(
                                    "{} · {}",
                                    short_text(&world.name, 14),
                                    human_age(world.modified)
                                )
                            }
                            None => "还没有世界".to_string(),
                        };
                        let content = if ring_data.update_names.is_empty() {
                            format!("{} 个 Mod · {} 个资源包", cache.mods, cache.packs)
                        } else {
                            format!("{} 个 Mod 可更新", ring_data.update_names.len())
                        };
                        let backup = match &ring_data.latest_snapshot {
                            Some(age) => format!("{} 个快照 · 最近 {age}", cache.snapshots),
                            None => "还没有快照".to_string(),
                        };
                        for (index, (name, detail)) in
                            [("最近世界", world), ("内容", content), ("备份", backup)]
                                .into_iter()
                                .enumerate()
                        {
                            let rect = egui::Rect::from_min_size(
                                cells.left_top() + egui::vec2(cell_w * index as f32, 0.0),
                                egui::vec2(cell_w, cells.height()),
                            );
                            if summary_cell(ui, rect, name, &detail, index > 0).clicked() {
                                match index {
                                    1 => open_content = true,
                                    _ => open_saves = true,
                                }
                            }
                        }

                        self.screenshot_strip(ui, &row, width);
                        ui.add_space(16.0);
                    });
                });
            });

        if let Some(id) = chosen {
            self.selected = Some(id);
        }
        match clicked_layer {
            Some(rings::Layer::Core) => primary = true,
            Some(rings::Layer::Files) => open_manage = true,
            Some(rings::Layer::Mods) => open_content = true,
            Some(rings::Layer::Saves) => open_saves = true,
            None => {}
        }
        if primary {
            if running {
                self.show_settings = true;
            } else if row.health.missing > 0 {
                self.start_repair(row.clone());
            } else {
                self.start_launch();
            }
        }
        if open_manage {
            self.nav_go(Nav::Versions);
        }
        if open_saves {
            self.open_saves(row.clone());
        }
        if open_content {
            self.open_content(true);
        }
    }

    fn delete_row(&mut self, row: &Row) {
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
                    .mods_row
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
                    let go = motion::secondary_sized(ui, "搜索", true, 54.0, 30.0, 10.5).clicked();
                    let response = search_field(ui, &mut self.mods_query, "名称或关键词", 200.0);
                    if std::mem::take(&mut self.focus_search) {
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
            self.mods_kind = kind;
            self.mods_category = None;
            self.mods_source_filter = None;
            self.mods_shelf = None;
            self.mods_searching = false;
            self.mods_filter.clear();
            self.mods_status = StatusFilter::All;
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
                        .size(13.0)
                        .color(t::sub()),
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
        if let Some(row) = &row {
            self.selected = Some(row.id.clone());
        }
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
        self.mods_status = StatusFilter::All;
        self.mods_recent.clear();
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
        self.mods_installed_dir = None;
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
        const COLLAPSED_CHIPS: usize = 8;
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
        if let Some(error) = self.mods_error.clone() {
            let mut reload = false;
            let empty = self.mods_browse.is_empty() && !self.mods_browse_loading;
            if empty {
                ui.add_space(34.0);
                ui.vertical_centered(|ui| {
                    egui::Frame::new()
                        .fill(t::surface())
                        .stroke(egui::Stroke::new(1.0, t::hairline()))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(24, 22))
                        .show(ui, |ui| {
                            ui.set_width(420.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("内容没有加载出来")
                                        .size(15.0)
                                        .family(bold_family()),
                                );
                                ui.add_space(5.0);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&error).size(13.0).color(t::sub()),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(12.0);
                                if motion::primary_sized(ui, "重新加载", true, 108.0, 32.0, 11.0)
                                {
                                    reload = true;
                                }
                            });
                        });
                });
            } else {
                egui::Frame::new()
                    .fill(t::tint(t::warn(), 0.11))
                    .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.41)))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(12, 9))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&error).size(13.0).color(t::accent()),
                                )
                                .wrap(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if action_link(ui, "重新加载").clicked() {
                                        reload = true;
                                    }
                                },
                            );
                        });
                    });
            }
            if reload {
                if self.mods_searching {
                    self.start_mods_search();
                } else {
                    self.start_browse();
                }
            }
            if empty {
                return;
            }
        }
        if self.mods_browse_loading && self.mods_browse.is_empty() {
            ui.add_space(16.0);
            egui::Frame::new()
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .inner_margin(egui::Margin::symmetric(20, 18))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        busy_spinner(ui);
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
                            .size(13.0)
                            .color(t::sub()),
                    );
                    ui.add_space(16.0);
                    for _ in 0..3 {
                        skeleton_line(ui, 12.0);
                        ui.add_space(8.0);
                    }
                });
            return;
        }

        let per_row = if ui.available_width() >= 800.0 {
            4
        } else if ui.available_width() >= 600.0 {
            3
        } else {
            2
        };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(185.0, 300.0);
        let shelves = std::mem::take(&mut self.mods_browse);
        if shelves.is_empty() && self.mods_error.is_none() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("这里还没有内容")
                        .size(13.0)
                        .color(t::sub()),
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
                                egui::RichText::new("更多 ›").size(13.0).color(t::sub()),
                            )
                            .fill(t::surface())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
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
            ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
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
                    ui.label(
                        egui::RichText::new(title)
                            .size(24.0)
                            .family(display_family())
                            .color(t::ink()),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} 个", self.mods_total))
                            .size(13.0)
                            .color(t::sub()),
                    );
                }
                None => {
                    ui.label(egui::RichText::new("搜索").size(13.0).color(t::sub()));
                    ui.label(
                        egui::RichText::new(format!(
                            "“{}”",
                            short_text(self.mods_query.trim(), 26)
                        ))
                        .size(15.0)
                        .family(bold_family()),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("{} 个结果", self.mods_total))
                            .size(13.0)
                            .color(t::sub()),
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
            ui.label(egui::RichText::new(warning).size(13.0).color(t::warn()));
        }
        if self.mods_loading {
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                busy_spinner(ui);
                ui.label(egui::RichText::new("正在加载…").size(13.0).color(t::sub()));
            });
        }
        let per_row = if ui.available_width() >= 800.0 {
            4
        } else if ui.available_width() >= 600.0 {
            3
        } else {
            2
        };
        let card_width = ((ui.available_width() - 10.0 * (per_row as f32 - 1.0)) / per_row as f32)
            .clamp(185.0, 300.0);
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
                .fill(t::surface())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
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
                            .size(13.0)
                            .color(t::sub()),
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
                        egui::Button::new(
                            egui::RichText::new("加载更多").size(13.0).color(t::sub()),
                        )
                        .fill(t::surface())
                        .stroke(egui::Stroke::new(1.0, t::hairline()))
                        .corner_radius(motion::R)
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
        let busy_reason = self.busy_reason();
        let busy = self.busy.is_some();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 146.0), egui::Sense::click());
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
        let hover_t = if motion::reduced() {
            if hovered { 1.0 } else { 0.0 }
        } else {
            ui.ctx()
                .animate_bool_with_time(response.id.with("card_hover"), hovered, 0.16)
        };
        let paint = rect;
        ui.painter().rect(
            paint,
            motion::R,
            motion::mix(t::surface(), t::wash(), hover_t),
            egui::Stroke::new(1.0, motion::mix(t::hairline(), t::control(), hover_t)),
            egui::StrokeKind::Inside,
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
                    .corner_radius(motion::R),
                );
            } else {
                let (icon_rect, _) = ui.allocate_exact_size(icon_size, egui::Sense::hover());
                ui.painter().rect_filled(icon_rect, motion::R, t::wash());
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
                    t::sub(),
                );
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(short_text(&project.name, 34))
                        .size(15.0)
                        .family(bold_family()),
                )
                .truncate(),
            );
        });
        child.add(
            egui::Label::new(
                egui::RichText::new(short_text(&project.summary, summary_chars))
                    .size(13.0)
                    .color(t::sub()),
            )
            .wrap(),
        );
        child.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{} 下载", compact(project.downloads)))
                    .size(13.0)
                    .color(t::sub()),
            );
            // 窄卡片放不下来源，避免压到右边的安装按钮。
            if width >= 240.0 {
                ui.label(egui::RichText::new("·").size(13.0).color(t::sub()));
                ui.label(
                    egui::RichText::new(project.source.label())
                        .size(13.0)
                        .color(t::sub()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if installed {
                    ui.label(egui::RichText::new("已装").size(13.0).color(t::ok()));
                } else if quick {
                    busy_spinner(ui);
                } else if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(
                            egui::RichText::new("+")
                                .size(15.0)
                                .family(bold_family())
                                .color(t::accent()),
                        )
                        .fill(t::tint(t::accent(), 0.16))
                        .stroke(egui::Stroke::new(1.0, t::tint(t::accent(), 0.78)))
                        .corner_radius(motion::R)
                        .min_size(egui::vec2(22.0, 22.0)),
                    )
                    .on_hover_text("安装到当前版本")
                    .on_disabled_hover_text(busy_reason)
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
        let busy_reason = self.busy_reason();
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
                            if let Some(texture) = self.mods_icons.get(&key) {
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
        if self.page_locked() {
            return;
        }
        self.selected = Some(row.id.clone());
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
        if enabled != self.backup_cfg.enabled {
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
                                    egui::RichText::new(&row.label).size(13.0).color(t::sub()),
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
                                    self.saves_worlds.len()
                                )
                            } else {
                                format!(
                                    "{} 个世界 · 快照分块去重，第二次只写改动块",
                                    self.saves_worlds.len()
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
                    if self.saves_busy {
                        busy_spinner(ui);
                        ui.label(
                            egui::RichText::new("正在写入快照…")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    }
                });
                if let Some(note) = &self.saves_notice {
                    ui.label(egui::RichText::new(note).size(13.0).color(t::ok()));
                }
                if let Some(err) = &self.saves_error {
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
                            egui::RichText::new(format!("{}", self.saves_worlds.len()))
                                .size(13.0)
                                .color(t::sub()),
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
                            ui.label(egui::RichText::new("还没有存档").size(13.0).color(t::sub()));
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
                let Some(world) = self.saves_selected.clone() else {
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
                    .saves_snapshots
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
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::secondary_sized(
                            ui,
                            "立即快照",
                            !self.saves_busy,
                            92.0,
                            32.0,
                            13.0,
                        )
                        .clicked()
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
                                .fill(t::surface())
                                .stroke(egui::Stroke::new(1.0, t::hairline()))
                                .corner_radius(motion::R)
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
        if (new_snapshot.is_some() || restore.is_some() || delete_world.is_some() || backup_all)
            && let Some(reason) =
                self.write_block(self.saves_row.as_ref().map(|row| row.id.as_str()))
        {
            self.saves_notice = None;
            self.saves_error = Some(if reason.starts_with("游戏") {
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
        let mut reveal: Option<PathBuf> = None;
        let mut open_rollback = false;

        let entries_all: Vec<installed::InstalledEntry> = self
            .mods_installed
            .iter()
            .filter(|entry| entry.kind == kind)
            .cloned()
            .collect();
        let enabled = entries_all.iter().filter(|entry| entry.enabled).count();
        let disabled = entries_all.len().saturating_sub(enabled);
        let updates: Vec<installed::UpdateCandidate> = self
            .mods_updates
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
        let status = self.mods_status;
        let query = self.mods_filter.trim().to_lowercase();
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
                &mut self.mods_filter,
                &if kind.label().is_ascii() {
                    format!("搜索 {} 名称或文件名", kind.label())
                } else {
                    format!("搜索{}名称或文件名", kind.label())
                },
                240.0,
            );
            if std::mem::take(&mut self.focus_search) {
                search.request_focus();
            }
            if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.mods_filter.clear();
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
            self.mods_status = filter;
            self.mods_delete_confirm = None;
        }

        if self.mods_updates_loading {
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
        if let Some(note) = &self.mods_notice {
            ui.add_space(9.0);
            ui.add(egui::Label::new(egui::RichText::new(note).size(13.0).color(t::ok())).wrap());
        }
        if let Some(err) = &self.mods_error {
            ui.add_space(9.0);
            ui.add(egui::Label::new(egui::RichText::new(err).size(13.0).color(t::danger())).wrap());
        }

        let loaded = !self.mods_installed_loading;
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
                        self.mods_status = StatusFilter::Duplicates;
                        self.mods_delete_confirm = None;
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
                                    format!("没有匹配「{}」的内容", self.mods_filter.trim())
                                }
                                (false, filter) => format!(
                                    "{}里没有匹配「{}」的",
                                    filter.label(),
                                    self.mods_filter.trim()
                                ),
                                (true, filter) => {
                                    format!("没有{}的{}", filter.label(), kind.label())
                                }
                            };
                            ui.label(egui::RichText::new(title).size(15.0).family(bold_family()));
                            ui.add_space(8.0);
                            if action_link(ui, "清除筛选，看全部").clicked() {
                                self.mods_filter.clear();
                                self.mods_status = StatusFilter::All;
                                self.mods_delete_confirm = None;
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
                                self.mods_delete_confirm.as_deref() == Some(entry.file.as_str());
                            let action = installed_row(
                                ui,
                                entry,
                                size,
                                candidate.map(|c| c.latest.version_number.as_str()),
                                confirming,
                                self.mods_recent.contains(&entry.file),
                                index + 1 == entries.len(),
                            );
                            if let Some(enabled) = action.toggle {
                                toggles.push((entry.clone(), enabled));
                            }
                            if action.reveal {
                                reveal = Some(installed::path_of(&game_dir, entry));
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
        let wants_write = import
            || !toggles.is_empty()
            || !deletes.is_empty()
            || update_all.is_some()
            || update_one.is_some()
            || open_rollback;
        if wants_write
            && let Some(reason) =
                self.write_block(self.mods_row.as_ref().map(|row| row.id.as_str()))
        {
            self.mods_notice = None;
            self.mods_error = Some(reason.into());
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
            self.mods_notice = None;
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
                                                        if self.java_ok {
                                                            "已检测到"
                                                        } else {
                                                            "未检测到"
                                                        }
                                                    ))
                                                    .size(13.0)
                                                    .color(if self.java_ok { t::ok() } else { t::warn() }),
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
        let mut snapshot_count = 0;
        let mut latest_snapshot: Option<u64> = None;
        for world in &worlds {
            let (count, latest) = snapshots::summary(game_dir, &world.name).unwrap_or((0, None));
            snapshot_count += count;
            latest_snapshot = latest_snapshot.max(latest);
        }
        let latest_world = worlds.pop();
        let mod_entries = index
            .entries
            .iter()
            .filter(|entry| entry.kind == mods::ContentKind::Mod)
            .map(|entry| (entry.file.clone(), entry.name.clone(), entry.enabled))
            .collect();
        let cache = HomeCache {
            row_id: row.id.clone(),
            mods: count(mods::ContentKind::Mod),
            packs: count(mods::ContentKind::ResourcePack),
            latest_world,
            mod_entries,
            snapshots: snapshot_count,
            latest_snapshot,
        };
        self.home_cache = Some(cache.clone());
        cache
    }

    /// 启动台先用索引里的数字，后台核对一遍内容目录（只看大小和修改时间，没变的不打开），
    /// 结果回来就更新。返回 true 表示刚更新过缓存。
    fn verify_home_content(&mut self, ctx: &egui::Context, row: &Row) -> bool {
        if let Some((id, rx)) = &self.home_scan {
            match rx.try_recv() {
                Ok(entries) => {
                    let id = id.clone();
                    self.home_scan = None;
                    if let Some(cache) = &mut self.home_cache
                        && cache.row_id == id
                    {
                        let count = |kind: mods::ContentKind| {
                            entries.iter().filter(|entry| entry.kind == kind).count()
                        };
                        cache.mods = count(mods::ContentKind::Mod);
                        cache.packs = count(mods::ContentKind::ResourcePack);
                        cache.mod_entries = entries
                            .iter()
                            .filter(|entry| entry.kind == mods::ContentKind::Mod)
                            .map(|entry| (entry.file.clone(), entry.name.clone(), entry.enabled))
                            .collect();
                        self.home_verified = Some(id);
                        return true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => self.home_scan = None,
            }
        }
        let fresh = self
            .home_cache
            .as_ref()
            .is_some_and(|cache| cache.row_id == row.id)
            && self.home_verified.as_deref() == Some(row.id.as_str());
        if fresh || self.busy.is_some() {
            return false;
        }
        let dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        let (tx, rx) = mpsc::channel();
        self.home_scan = Some((row.id.clone(), rx));
        self.home_verified = Some(row.id.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let entries = if dir.is_dir() {
                installed::scan_all(&dir)
                    .map(|index| index.entries)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let _ = tx.send(entries);
            ctx.request_repaint();
        });
        false
    }

    /// 把缓存和当前任务整理成状态圆环要显示的数据。
    fn ring_data(&self, row: &Row, cache: &HomeCache) -> rings::Data {
        let updates: Vec<&installed::UpdateCandidate> = if self
            .mods_row
            .as_ref()
            .is_some_and(|target| target.id == row.id)
        {
            self.mods_updates.iter().collect()
        } else {
            Vec::new()
        };
        let mods = cache
            .mod_entries
            .iter()
            .map(|(file, _, enabled)| rings::ModDot {
                enabled: *enabled,
                update: updates.iter().any(|update| update.entry.file == *file),
            })
            .collect();
        let update_names = updates
            .iter()
            .map(|update| update.entry.name.clone())
            .collect();
        let missing = row.health.missing;
        let state = if matches!(self.busy, Some(Task::Launch)) {
            rings::State::Running
        } else if missing > 0 {
            rings::State::Missing
        } else {
            rings::State::Ready
        };
        rings::Data {
            state,
            // 健康检查没有总文件数，这里只表达“缺得越多缺口越大”。
            fraction: 1.0 - missing as f32 / (missing as f32 + 60.0),
            missing: missing as usize,
            progress_detail: String::new(),
            mods,
            update_names,
            snapshots: cache.snapshots,
            latest_snapshot: cache.latest_snapshot.map(human_age),
            running_minutes: self
                .launch_started
                .map(|started| started.elapsed().as_secs() / 60)
                .unwrap_or(0),
        }
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
                                                    if self.catalog_loading {
                                                        busy_spinner(ui);
                                                        ui.label(
                                                            egui::RichText::new("获取版本…")
                                                                .size(13.0)
                                                                .color(t::sub()),
                                                        );
                                                    } else if let Some(err) = &self.catalog_error {
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
                                                    if self.java_ok {
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
                        if !self.java_ok {
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
        let bench = matches!(self.busy, Some(Task::Bench));
        let title = match self.busy {
            Some(Task::Mods) => "正在下载内容",
            Some(Task::Bench) => "正在测试",
            _ => "正在安装",
        };
        let width = ui.available_width().min(520.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        let now = ui.input(|input| input.time);
        let shown = ui.ctx().animate_value_with_time(
            egui::Id::unique("progress_value"),
            p.fraction(),
            0.30,
        );
        let compact_h = ui.ctx().content_rect().height() < 680.0;
        let ring_h: f32 = if compact_h { 200.0 } else { 260.0 };
        let files = if bench {
            "后台跑模拟负载，不开游戏窗口；期间电脑会比较忙".to_string()
        } else if p.files_total > 0 {
            format!("{} / {} 个文件", p.files_done, p.files_total)
        } else {
            format!("{} 个文件", p.files_done)
        };
        let progress_detail = if p.bytes_total > 0 {
            format!("{files}，{}", progress_detail(&p))
        } else {
            files.clone()
        };
        let data = rings::Data {
            state: rings::State::Installing,
            fraction: if p.indeterminate { 0.0 } else { shown },
            missing: 0,
            progress_detail,
            mods: self
                .home_cache
                .as_ref()
                .map(|cache| vec![rings::ModDot::default(); cache.mod_entries.len()])
                .unwrap_or_default(),
            update_names: Vec::new(),
            snapshots: self.home_cache.as_ref().map_or(0, |cache| cache.snapshots),
            latest_snapshot: self
                .home_cache
                .as_ref()
                .and_then(|cache| cache.latest_snapshot.map(human_age)),
            running_minutes: 0,
        };
        let mut cancel_clicked = false;
        ui.add_space(if compact_h { 10.0 } else { 24.0 });
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
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    if !p.indeterminate {
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", shown * 100.0))
                                .size(44.0)
                                .family(light_family()),
                        );
                    }
                    ui.label(
                        egui::RichText::new(format!("{title} · {}", self.progress_label))
                            .size(14.0),
                    );
                    ui.label(egui::RichText::new(&p.stage).size(13.0).color(t::sub()));
                });
                ui.add_space(10.0);
                motion::progress_bar(ui, shown, p.indeterminate, now);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&files).size(13.0).color(t::dim()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if p.bytes_total > 0 {
                            ui.label(
                                egui::RichText::new(self::progress_detail(&p))
                                    .size(13.0)
                                    .color(t::dim()),
                            );
                        }
                    });
                });
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    let label = match (self.cancel_requested, bench) {
                        (true, _) => "正在取消…",
                        (false, true) => "停止测试",
                        (false, false) => "取消安装",
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
                            "已完成的部分会保留，下次只补缺失文件"
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        t::sync(ui.ctx());
        self.poll();
        motion::set_reduced(self.reduce_motion);
        self.handle_shortcuts(ui.ctx());
        self.drain_icons(ui.ctx());
        if self.backup_cfg.enabled {
            ui.ctx().request_repaint_after(Duration::from_secs(5));
            self.maybe_auto_backup();
        }
        let dropped: Vec<PathBuf> = ui
            .ctx()
            .input_mut(|i| std::mem::take(&mut i.raw.dropped_files))
            .into_iter()
            .map(|file| file.path().to_path_buf())
            .collect();
        // 安装页有自己的整合包拖放区，那里只认整合包。
        if !dropped.is_empty() {
            self.drop_preview = None;
            let plan = self.plan_drop(&dropped);
            if self.show_install && plan.items.is_empty() && plan.packs.len() == 1 {
                if self.busy.is_none() {
                    self.start_modpack(plan.packs[0].clone());
                }
            } else {
                self.import_dropped(plan);
            }
        }
        // 回到窗口时，文件管理器里做的改动直接反映出来，不用点「重新扫描」。
        let focused = ui.ctx().input(|i| i.focused);
        if focused && !self.window_focused {
            self.rescan_if_changed();
        }
        self.window_focused = focused;
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
            // 别的页面可能改了内容或快照，回到启动台时重新读取。
            self.home_cache = None;
            self.home_verified = None;
            self.tuning_cache.clear();
            self.page_enter_at = now;
        }
        let enter = if self.reduce_motion {
            1.0
        } else {
            (((now - self.page_enter_at) / 0.15) as f32).clamp(0.0, 1.0)
        };
        let eased = motion::ease_out_cubic(enter);
        if enter < 1.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_HOVER_CYCLE").is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        self.sidebar(ui);
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(t::bg()))
            .show(ui, |ui| {
                ui.add_space((1.0 - eased) * 6.0);
                if matches!(
                    self.busy,
                    Some(Task::Install) | Some(Task::Mods) | Some(Task::Bench)
                ) {
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
                // 版本、内容、存档页没有放全局提示的位置，统一放在底部。
                if matches!(page, 2..=8) {
                    self.message_bar(ui);
                }
                self.drop_overlay(ui);
                // 用一层底色做淡入：嵌套面板各有画笔，逐个调透明度并不可靠。
                let veil = 1.0 - eased;
                if veil > 0.001 {
                    ui.painter()
                        .rect_filled(ui.max_rect(), 0.0, t::tint(t::bg(), veil));
                }
            });
        self.account_overlay(ui.ctx());
        self.settings_overlay(ui.ctx());
        self.bench_overlay(ui.ctx());
        self.rollback_overlay(ui.ctx());
        self.install_review_overlay(ui.ctx());
        self.log_overlay(ui.ctx());
        self.launch_overlay(ui.ctx());
        if let Some(shots) = &mut self.shots {
            shots.viewer(ui.ctx());
        }
        motion::focus_ring(ui.ctx());
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
/// Four-block pixel spinner. Used anywhere the old egui arc spinner was;
/// it keeps the whole shell on the same grid and only requests frames while
/// it is actually visible.
fn busy_spinner(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    let time = if motion::reduced() {
        0.0
    } else {
        ui.input(|i| i.time) as f32
    };
    ui.painter()
        .circle_stroke(rect.center(), 6.0, egui::Stroke::new(1.0, t::hairline()));
    let points: Vec<egui::Pos2> = (0..=12)
        .map(|index| {
            let angle = time * 4.0 + index as f32 / 12.0 * 1.8;
            rect.center() + egui::vec2(angle.cos(), angle.sin()) * 6.0
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.4, t::accent()),
    ));
    if !motion::reduced() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

fn content_tab(ui: &mut egui::Ui, label: &str, count: usize, active: bool) -> egui::Response {
    let text = if count > 0 {
        format!("{label}  {count}")
    } else {
        label.to_string()
    };
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(14.0).color(if active {
            t::ink()
        } else {
            t::sub()
        }))
        .fill(if active {
            t::wash()
        } else {
            egui::Color32::TRANSPARENT
        })
        .stroke(egui::Stroke::NONE)
        .corner_radius(motion::R)
        .min_size(egui::vec2(if count > 0 { 92.0 } else { 68.0 }, 32.0)),
    )
}

/// 品类胶囊 / 已安装切换用的小按钮。
fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let (fill, color, border) = if selected {
        (t::wash(), t::ink(), t::control())
    } else {
        (egui::Color32::TRANSPARENT, t::sub(), t::hairline())
    };
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(color))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, border))
            .corner_radius(motion::R)
            .min_size(egui::vec2(0.0, 28.0)),
    )
}

/// 启动台大标题的字号：64 号放得下就用 64，放不下按宽度缩小，最小 30；
/// 30 号还放不下就换行（返回 true）。
fn fit_title(ui: &egui::Ui, text: &str, max_width: f32) -> (f32, bool) {
    const MAX: f32 = 64.0;
    const MIN: f32 = 30.0;
    let measured = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_string(),
            egui::FontId::new(MAX, light_family()),
            egui::Color32::WHITE,
        )
        .size()
        .x
    });
    if measured <= max_width {
        return (MAX, false);
    }
    let size = (MAX * max_width / measured * 0.98).floor();
    if size >= MIN {
        (size, false)
    } else {
        // 两行能放下的最大字号，留一点余量给断行位置。
        (
            (MAX * 2.0 * max_width / measured * 0.9)
                .floor()
                .clamp(24.0, MIN),
            true,
        )
    }
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

fn feature(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).size(13.0).family(bold_family()));
        ui.label(egui::RichText::new(detail).size(13.0).color(t::sub()));
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

fn quick_action(ui: &mut egui::Ui, text: &str) -> egui::Response {
    motion::secondary_sized(ui, text, true, 94.0, 30.0, 11.0)
}

fn action_link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE),
    )
}

fn avatar_sized(ui: &mut egui::Ui, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size * 0.5, t::wash());
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.46),
        t::sub(),
    );
}

fn status_dot(ui: &mut egui::Ui, ok: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 3.5, if ok { t::ok() } else { t::warn() });
}

fn inline_dot(ui: &mut egui::Ui) {
    ui.add_space(7.0);
    ui.label(egui::RichText::new("·").size(13.0).color(t::sub()));
    ui.add_space(7.0);
}

fn stat_cell(ui: &mut egui::Ui, value: usize, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(96.0, 42.0),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            ui.set_min_height(42.0);
            ui.label(
                egui::RichText::new(value.to_string())
                    .size(15.0)
                    .family(bold_family()),
            );
            ui.label(egui::RichText::new(label).size(13.0).color(t::sub()));
        },
    );
}

fn info_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.0).color(t::sub()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).size(13.0));
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

/// 列表行的背景：当前项和悬停项都填 `wash`，不加色条。
fn list_row_fill(ui: &egui::Ui, rect: egui::Rect, active: bool, hovered: bool) {
    if active {
        ui.painter().rect_filled(rect, motion::R, t::wash());
    } else if hovered {
        ui.painter()
            .rect_filled(rect, motion::R, t::tint(t::wash(), 0.7));
    }
}

fn version_row(ui: &mut egui::Ui, row: &Row, active: bool) -> egui::Response {
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

/// 启动台的一栏摘要。
fn summary_cell(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    title: &str,
    detail: &str,
    divider: bool,
) -> egui::Response {
    let response = ui.interact(
        rect,
        egui::Id::unique("summary_cell").with(title),
        egui::Sense::click(),
    );
    let hover = motion::hover_amount(ui, response.id.with("hover"), response.hovered());
    if hover > 0.0 {
        ui.painter()
            .rect_filled(rect.shrink(4.0), motion::R, t::tint(t::wash(), hover));
    }
    if divider {
        ui.painter().line_segment(
            [rect.left_top(), rect.left_bottom()],
            egui::Stroke::new(1.0, t::hairline()),
        );
    }
    let left = rect.left() + if divider { 18.0 } else { 8.0 };
    ui.painter().text(
        egui::pos2(left, rect.top() + 18.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::new(15.0, bold_family()),
        t::ink(),
    );
    ui.painter().text(
        egui::pos2(left, rect.top() + 44.0),
        egui::Align2::LEFT_TOP,
        detail,
        egui::FontId::proportional(13.0),
        t::sub(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(1.0, t::hairline()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::same(18))
}

/// 首次启动的步骤编号：完成的一步用绿色，未完成用暖色。
fn onboarding_badge(ui: &mut egui::Ui, number: &str, done: bool) {
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

/// 加载骨架：不伪装成真实内容，只给一个稳定的占位高度。
fn skeleton_line(ui: &mut egui::Ui, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, height / 2.0, t::wash());
}

/// 安装确认里的一行「标签 + 值」。长路径省略显示，悬停可看全。
fn install_review_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [64.0, 16.0],
            egui::Label::new(egui::RichText::new(label).size(13.0).color(t::sub())),
        );
        ui.add(egui::Label::new(egui::RichText::new(value).size(13.0)).truncate())
            .on_hover_text(value);
    });
}

fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(58.0, 28.0)),
    )
}

fn pick_content_files(kind: mods::ContentKind) -> Vec<PathBuf> {
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

/// 例如「3 个 Mod」「2 个 Mod、1 个资源包」。
fn count_kinds(kinds: impl Iterator<Item = mods::ContentKind>) -> String {
    let mut counts: Vec<(mods::ContentKind, usize)> = Vec::new();
    for kind in kinds {
        match counts.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((kind, 1)),
        }
    }
    counts
        .iter()
        .map(|(kind, n)| {
            let label = kind.label();
            if label.is_ascii() {
                format!("{n} 个 {label}")
            } else {
                format!("{n} 个{label}")
            }
        })
        .collect::<Vec<_>>()
        .join("、")
}

/// 三个内容目录的文件名、大小和修改时间合成一个数，用来发现外面的改动。
fn content_signature(game_dir: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for kind in [
        mods::ContentKind::Mod,
        mods::ContentKind::ResourcePack,
        mods::ContentKind::Shader,
    ] {
        let mut items: Vec<(String, u64, u64)> = std::fs::read_dir(game_dir.join(kind.folder()))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                Some((
                    entry.file_name().to_string_lossy().to_string(),
                    meta.len(),
                    modified,
                ))
            })
            .collect();
        items.sort();
        items.hash(&mut hasher);
    }
    hasher.finish()
}

fn pick_modpack() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("选择整合包")
        .add_filter("整合包", &["mrpack", "zip"])
        .pick_file()
}

/// 侧边栏的一级入口：宽窗口带文字，窄窗口只有图标。
fn nav_row(
    ui: &mut egui::Ui,
    height: f32,
    active: bool,
    enabled: bool,
) -> (egui::Rect, egui::Response, egui::Color32) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hover = motion::hover_amount(ui, response.id.with("hover"), enabled && response.hovered());
    if active {
        ui.painter().rect_filled(rect, motion::R, t::wash());
    } else if hover > 0.0 {
        ui.painter()
            .rect_filled(rect, motion::R, t::tint(t::wash(), hover));
    }
    let color = if !enabled {
        t::dim()
    } else if active {
        t::ink()
    } else {
        motion::mix(t::sub(), t::ink(), hover)
    };
    (rect, response, color)
}

/// 侧边栏的一级入口：宽窗口带文字，窄窗口只有图标。
fn sidebar_item(
    ui: &mut egui::Ui,
    nav: Nav,
    active: bool,
    wide: bool,
    enabled: bool,
) -> egui::Response {
    let (rect, response, color) = nav_row(ui, 36.0, active, enabled);
    let icon = if wide {
        egui::pos2(rect.left() + 21.0, rect.center().y)
    } else {
        rect.center()
    };
    nav_glyph(ui.painter(), icon, nav, color);
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            nav.label(),
            egui::FontId::proportional(14.0),
            color,
        );
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response.on_hover_text("任务进行中，完成后再切换")
    }
    .on_hover_text(if wide { nav.hint() } else { nav.label() })
}

fn nav_glyph(painter: &egui::Painter, c: egui::Pos2, nav: Nav, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    let p = |x: f32, y: f32| c + egui::vec2(x, y);
    match nav {
        Nav::Home => {
            painter.add(egui::Shape::closed_line(
                vec![p(-4.5, -7.0), p(7.0, 0.0), p(-4.5, 7.0)],
                stroke,
            ));
        }
        Nav::Versions => {
            for y in [-4.0, 0.0, 4.0] {
                painter.add(egui::Shape::closed_line(
                    vec![p(0.0, y - 3.5), p(7.5, y), p(0.0, y + 3.5), p(-7.5, y)],
                    stroke,
                ));
            }
        }
        Nav::Content => {
            for (x, y) in [(-7.0, -7.0), (1.0, -7.0), (-7.0, 1.0), (1.0, 1.0)] {
                painter.rect_stroke(
                    egui::Rect::from_min_size(p(x, y), egui::vec2(6.0, 6.0)),
                    1.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Nav::Saves => {
            painter.circle_stroke(c, 7.5, stroke);
            painter.line_segment([p(-7.5, 0.0), p(7.5, 0.0)], stroke);
            painter.add(egui::Shape::ellipse_stroke(c, egui::vec2(3.2, 7.5), stroke));
        }
    }
}

fn sidebar_account(ui: &mut egui::Ui, name: &str, linked: bool, wide: bool) -> egui::Response {
    let (rect, response, color) = nav_row(ui, 36.0, false, true);
    let center = if wide {
        egui::pos2(rect.left() + 21.0, rect.center().y)
    } else {
        rect.center()
    };
    ui.painter().circle_stroke(
        center + egui::vec2(0.0, -3.0),
        3.2,
        egui::Stroke::new(1.4, color),
    );
    ui.painter().add(egui::Shape::line(
        (0..=10)
            .map(|index| {
                let angle = std::f32::consts::PI * (1.0 + index as f32 / 10.0);
                center + egui::vec2(6.5 * angle.cos(), 7.0 + 6.0 * angle.sin())
            })
            .collect(),
        egui::Stroke::new(1.4, color),
    ));
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            short_text(name, 14),
            egui::FontId::proportional(14.0),
            color,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if linked {
            "Microsoft 正版账户"
        } else {
            "离线账户，点击登录 Microsoft"
        })
}

/// 侧边栏底部的工具入口：设置不是一级页面，做成安静的滑杆图标。
fn sidebar_settings(ui: &mut egui::Ui, wide: bool, enabled: bool) -> egui::Response {
    let (rect, response, color) = nav_row(ui, 36.0, false, enabled);
    let center = if wide {
        egui::pos2(rect.left() + 21.0, rect.center().y)
    } else {
        rect.center()
    };
    let stroke = egui::Stroke::new(1.4, color);
    for (y, x) in [(-4.0, -2.5), (0.0, 3.0), (4.0, -0.5)] {
        ui.painter().line_segment(
            [center + egui::vec2(-7.5, y), center + egui::vec2(7.5, y)],
            stroke,
        );
        ui.painter()
            .circle(center + egui::vec2(x, y), 2.0, t::bg(), stroke);
    }
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "设置",
            egui::FontId::proportional(14.0),
            color,
        );
    }
    if enabled {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("设置：外观、内存、Java、目录与日志（Ctrl+,）")
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

/// 在文件管理器里打开所在目录并选中这个文件；做不到时退回打开目录。
fn reveal_file(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // freedesktop 的 FileManager1 接口能选中文件（Dolphin、Nautilus、Nemo 都支持）。
        let uri = format!("file://{}", percent_path(path));
        let shown = std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !shown && let Some(parent) = path.parent() {
            open_folder(parent);
        }
    }
}

/// 文件 URI 里的路径要百分号编码：中文目录名和空格都很常见。
#[cfg(all(unix, not(target_os = "macos")))]
fn percent_path(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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

/// 内容接口的原始错误常带完整 URL；界面上只留一句可行动的话。
fn content_error(message: String) -> String {
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

fn escape_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Escape))
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

fn toggle_switch(ui: &mut egui::Ui, on: bool) -> egui::Response {
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
struct RowAction {
    toggle: Option<bool>,
    reveal: bool,
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

fn primary_wide(ui: &mut egui::Ui, text: &str, enabled: bool, width: f32) -> bool {
    motion::primary_sized(ui, text, enabled, width, 40.0, 13.5)
}

fn accent_outline(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(text)
                .size(13.0)
                .family(bold_family())
                .color(t::accent()),
        )
        .fill(t::tint(t::accent(), 0.16))
        .stroke(egui::Stroke::new(1.0, t::tint(t::accent(), 0.86)))
        .corner_radius(motion::R)
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

fn text_field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(9, 4)),
            ),
    )
}

/// 按宽度从中间省略：路径的开头（盘符、用户目录）和结尾（具体是哪个版本）都留着。
fn elide_middle(ui: &egui::Ui, text: &str, font: &egui::FontId, max_width: f32) -> String {
    let width = |s: &str| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
        })
    };
    if width(text) <= max_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let keep = (lo + hi).div_ceil(2);
        let head = keep * 2 / 5;
        let tail = keep - head;
        let candidate: String = chars[..head]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(&chars[chars.len() - tail..])
            .collect();
        if width(&candidate) <= max_width {
            lo = keep;
        } else {
            hi = keep - 1;
        }
    }
    let head = lo * 2 / 5;
    let tail = lo - head;
    chars[..head]
        .iter()
        .chain(std::iter::once(&'…'))
        .chain(&chars[chars.len() - tail..])
        .collect()
}

/// 「标签 · 路径 · 打开」一行：标签和按钮宽度固定，路径占剩下的地方，太长从中间省略；
/// 悬停看全路径，点一下复制。返回「打开」是否被点了。
fn path_row(ui: &mut egui::Ui, label: &str, path: &std::path::Path) -> bool {
    let full = path.display().to_string();
    let font = egui::FontId::proportional(13.0);
    let mut open = false;
    ui.horizontal(|ui| {
        let row_w = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(64.0, 24.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(64.0);
                ui.label(egui::RichText::new(label).size(13.0).color(t::sub()));
            },
        );
        let path_w = (row_w - 64.0 - 54.0 - 16.0).max(60.0);
        let shown = elide_middle(ui, &full, &font, path_w);
        let response = ui
            .allocate_ui_with_layout(
                egui::vec2(path_w, 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(path_w);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&shown)
                                .font(font.clone())
                                .color(t::sub()),
                        )
                        .sense(egui::Sense::click()),
                    )
                },
            )
            .inner;
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
        }
        let copied_id = egui::Id::unique(("path_copied", label));
        if response.clicked() {
            ui.ctx().copy_text(full.clone());
            ui.ctx()
                .data_mut(|d| d.insert_temp(copied_id, ui.input(|i| i.time)));
        }
        let copied = ui
            .ctx()
            .data(|d| d.get_temp::<f64>(copied_id))
            .is_some_and(|at| ui.input(|i| i.time) - at < 1.5);
        response.on_hover_text(if copied {
            format!("已复制\n{full}")
        } else {
            format!("{full}\n点一下复制")
        });
        if motion::secondary_sized(ui, "打开", true, 54.0, 24.0, 10.0).clicked() {
            open = true;
        }
    });
    open
}

/// 搜索框：左边一个放大镜，有内容时右边一个清除按钮。
fn search_field(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .desired_width(width)
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin {
                        left: 28,
                        right: 24,
                        top: 5,
                        bottom: 5,
                    }),
            ),
    );
    let rect = response.rect;
    let color = if response.has_focus() {
        t::sub()
    } else {
        t::dim()
    };
    let c = egui::pos2(rect.left() + 14.0, rect.center().y - 1.0);
    let painter = ui.painter();
    painter.circle_stroke(c, 4.5, egui::Stroke::new(1.3, color));
    painter.line_segment(
        [c + egui::vec2(3.3, 3.3), c + egui::vec2(6.6, 6.6)],
        egui::Stroke::new(1.5, color),
    );
    if !text.is_empty() {
        let clear = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            egui::vec2(18.0, 18.0),
        );
        if ui
            .put(
                clear,
                egui::Button::new(egui::RichText::new("×").size(14.0).color(t::dim())).frame(false),
            )
            .on_hover_text("清除（Esc）")
            .clicked()
        {
            text.clear();
        }
    }
    response
}

/// 状态筛选胶囊：「已禁用 3」。有颜色的数字提醒这里有事，点了就只看这些。
fn status_chip(
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

/// 内存：「自动」加 1 GB 步进。自动时显示算出来的值，按加减就转成手动。
fn memory_stepper(ui: &mut egui::Ui, memory: &mut Option<u32>, auto_mb: u32) {
    if chip(ui, "自动", memory.is_none()).clicked() {
        *memory = None;
    }
    ui.add_space(4.0);
    let (down, up) = match *memory {
        Some(mb) => {
            let gb = (mb as f32 / 1024.0).round() as u32;
            (gb.saturating_sub(1), gb + 1)
        }
        None => (auto_mb.div_ceil(1024).saturating_sub(1), auto_mb / 1024 + 1),
    };
    let button = |ui: &mut egui::Ui, text: &str, enabled: bool| {
        ui.add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
                .fill(t::bg())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .min_size(egui::vec2(28.0, 28.0)),
        )
        .clicked()
    };
    if button(ui, "−", down >= 1) {
        *memory = Some(down * 1024);
    }
    let (text, color) = match *memory {
        Some(mb) => (tuning::gb(mb), t::ink()),
        None => (tuning::gb(auto_mb), t::sub()),
    };
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(1.0, t::hairline()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.add_sized(
                [52.0, 18.0],
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(13.0)
                        .color(color)
                        .family(bold_family()),
                ),
            );
        });
    if button(ui, "+", up <= 16) {
        *memory = Some(up * 1024);
    }
}

fn detail_kv_label(ui: &mut egui::Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(58.0, 16.0), egui::Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        t::sub(),
    );
}

fn detail_kv(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        detail_kv_label(ui, label);
        ui.add(egui::Label::new(egui::RichText::new(value).size(13.0).color(t::ink())).truncate());
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

/// Exact-size pill for toolbars and modal corners. Unlike `tag_fill` it
/// refuses to stretch to the height of the surrounding row.
fn tag_pill(ui: &mut egui::Ui, text: &str, color: egui::Color32, fill: egui::Color32) {
    let font = egui::FontId::new(13.0, egui::FontFamily::Proportional);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2((galley.size().x + 14.0).max(30.0), 20.0),
        egui::Sense::hover(),
    );
    ui.painter().rect(
        rect,
        motion::R,
        fill,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 46),
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
}

fn tag(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    tag_fill(ui, text, color, t::wash());
}

fn tag_fill(ui: &mut egui::Ui, text: &str, color: egui::Color32, fill: egui::Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(13.0).color(color));
        });
}

fn small_accent(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::accent()))
            .fill(t::tint(t::accent(), 0.14))
            .stroke(egui::Stroke::new(1.0, t::tint(t::accent(), 0.71)))
            .corner_radius(motion::R)
            .min_size(egui::vec2(44.0, 24.0)),
    )
}

fn tiny_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
            .fill(t::bg())
            .stroke(egui::Stroke::new(1.0, t::hairline()))
            .corner_radius(motion::R)
            .min_size(egui::vec2(44.0, 24.0)),
    )
}

fn danger_tiny(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::danger()))
            .fill(t::tint(t::danger(), 0.14))
            .corner_radius(motion::R)
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

/// 中文正文与粗体分成两个字体族；`RichText::family(bold_family())` 才是真粗体。
pub(crate) fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// 页面标题用常规字重，层级靠字号表达。
pub(crate) fn display_family() -> egui::FontFamily {
    egui::FontFamily::Proportional
}

/// 启动台版本号这类大字用的细体。只有拉丁字符有细体，中文回退到常规字重。
pub(crate) fn light_family() -> egui::FontFamily {
    egui::FontFamily::Name("light".into())
}

fn install_fonts(ctx: &egui::Context) {
    // 中文用系统字体，不内置。Noto 的 ttc 里第 2 个字形集才是简体。
    const REGULAR: [(&str, u32); 6] = [
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
        (
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            2,
        ),
        (
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            0,
        ),
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),
        ("/System/Library/Fonts/PingFang.ttc", 0),
    ];
    const BOLD: [(&str, u32); 5] = [
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc", 2),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc", 2),
        ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Bold.ttc", 2),
        ("/usr/share/fonts/opentype/noto/NotoSansCJKsc-Bold.otf", 0),
        ("C:\\Windows\\Fonts\\msyhbd.ttc", 0),
    ];
    let mut fonts = egui::FontDefinitions::default();
    let mut load = |key: &str, candidates: &[(&str, u32)]| -> bool {
        for (path, index) in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = *index;
                fonts.font_data.insert(key.to_owned(), data.into());
                return true;
            }
        }
        false
    };
    let has_regular = load("cjk", &REGULAR);
    let has_bold = load("cjk-bold", &BOLD);
    let defaults = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut normal = Vec::new();
    if has_regular {
        normal.push("cjk".to_owned());
    }
    normal.extend(defaults);
    let mut bold = normal.clone();
    if has_bold {
        bold.insert(0, "cjk-bold".to_owned());
    }
    // Ubuntu Light 随 egui 默认字体一起分发，不增加体积。
    let mut light = vec!["Ubuntu-Light".to_owned()];
    light.extend(normal.clone());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.insert(family, normal.clone());
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("bold".into()), bold);
    fonts
        .families
        .insert(egui::FontFamily::Name("light".into()), light);
    ctx.set_fonts(fonts);
}

/// 窗口和任务栏图标：和 exe、deb 用同一张图。
fn app_icon() -> egui::IconData {
    const PNG: &[u8] =
        include_bytes!("../../../packaging/linux/icons/io.github.dkjsiogu.cairn-256.png");
    match image::load_from_memory(PNG) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
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
            .with_title("Cairn")
            .with_app_id("cairn")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "cairn",
        options,
        Box::new(move |cc| {
            t::install(&cc.egui_ctx);
            install_fonts(&cc.egui_ctx);
            root.migrate_legacy_cache();
            let app = App::new(root);
            app.theme_pref.apply(&cc.egui_ctx);
            Ok(Box::new(app))
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
