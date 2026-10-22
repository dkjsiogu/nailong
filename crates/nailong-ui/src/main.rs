#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::install::{catalog, fabric, forge, health, modpack, neoforge, vanilla};
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::mods;
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::{Error, Root, cancel, java};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(242, 193, 78);
const CARD: egui::Color32 = egui::Color32::from_rgb(24, 27, 33);
const PANEL: egui::Color32 = egui::Color32::from_rgb(20, 22, 27);
const LINE: egui::Color32 = egui::Color32::from_rgb(42, 46, 56);
const MUTED: egui::Color32 = egui::Color32::from_rgb(145, 148, 156);
const WARN: egui::Color32 = egui::Color32::from_rgb(242, 160, 90);

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
    LaunchDone(i32),
    LaunchFailed(String),
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
    mods_source: Option<mods::Source>,
    mods_results: Vec<mods::Project>,
    mods_total: u64,
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
    rx: Option<mpsc::Receiver<Msg>>,
    catalog_rx: Option<mpsc::Receiver<Msg>>,
}

impl App {
    fn new(root: Root) -> Self {
        let java_ok = java::find_all().iter().any(|j| j.major >= 21);
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
            mods_source: None,
            mods_results: Vec::new(),
            mods_total: 0,
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
            rx: None,
            catalog_rx: None,
        };
        app.refresh();
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
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let opts = LaunchOptions {
                username,
                memory_mb: memory,
                game_dir: row.game_dir.clone(),
                ..Default::default()
            };
            let msg = match launch::launch(&root, &row.profile, &opts) {
                Ok(0) => Msg::LaunchDone(0),
                Ok(code) => Msg::LaunchDone(code),
                Err(e) => Msg::LaunchFailed(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    fn open_mods(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.selected_row() else {
            self.error = Some("先安装一个 Fabric / NeoForge / Forge 版本".into());
            return;
        };
        if mod_loader(row.loader).is_none() {
            self.error = Some("原版没有 mods 目录，请先安装 Fabric / NeoForge / Forge".into());
            return;
        }
        let game_dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        self.mods_row = Some(row);
        self.mods_game_dir = Some(game_dir);
        self.show_mods = true;
        self.show_install = false;
        self.show_manage = false;
        self.mods_query.clear();
        self.mods_source = None;
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
        self.error = None;
        self.notice = None;
    }

    fn close_mods(&mut self) {
        self.show_mods = false;
        self.mods_row = None;
        self.mods_game_dir = None;
        self.mods_rx = None;
        self.mods_loading = false;
        self.mods_deps_loading = false;
        self.mods_notice = None;
        self.mods_error = None;
    }

    fn start_mods_search(&mut self) {
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let Some(loader) = mod_loader(row.loader) else {
            return;
        };
        let query = self.mods_query.trim().to_string();
        self.mods_loading = true;
        self.mods_error = None;
        self.mods_notice = None;
        self.mods_warnings.clear();
        self.mods_results.clear();
        self.mods_total = 0;
        self.mods_selected = None;
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        let source = self.mods_source;
        let mc = row.mc.clone();
        let (tx, rx) = mpsc::channel();
        self.mods_rx = Some(rx);
        std::thread::spawn(move || {
            let result =
                mods::search(&query, source, &mc, loader, 0, 20).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ModsSearch { query, result });
        });
    }

    fn select_mods_project(&mut self, project: mods::Project) {
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let Some(loader) = mod_loader(row.loader) else {
            return;
        };
        self.mods_selected = Some(project.clone());
        self.mods_versions.clear();
        self.mods_version = None;
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_deps_loading = false;
        self.mods_error = None;
        self.mods_notice = None;
        let project_key = format!("{:?}:{}", project.source, project.id);
        let mc = row.mc.clone();
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
        let Some(row) = self.mods_row.clone() else {
            return;
        };
        let Some(loader) = mod_loader(row.loader) else {
            return;
        };
        self.mods_version = Some(version.clone());
        self.mods_choices.clear();
        self.mods_optional.clear();
        self.mods_deps_loading = true;
        self.mods_error = None;
        self.mods_notice = None;
        let version_key = format!("{:?}:{}", version.source, version.id);
        let mc = row.mc.clone();
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
        let Some(loader) = mod_loader(row.loader) else {
            return;
        };
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
        let mods_dir = game_dir.join("mods");
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
        self.progress = Some(Progress::stage("解析 Mod 依赖"));
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
            let result = mods::plan(&version, &mc, loader, &optional)
                .and_then(|plan| mods::download_plan(&mods_dir, &plan, false, &sink));
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
        let mut messages = Vec::new();
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
                Msg::ModsSearch { query, result } => {
                    self.mods_loading = false;
                    self.mods_rx = None;
                    if query != self.mods_query.trim() {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            self.mods_results = page.hits;
                            self.mods_total = page.total;
                            self.mods_warnings = page.warnings;
                            self.mods_error = None;
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
                    self.mods_notice = Some(note);
                    self.mods_error = None;
                    self.refresh();
                }
                Msg::ModsFailed(e) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.mods_error = Some(e);
                }
                Msg::ModsCancelled => {
                    self.busy = None;
                    self.cancel_requested = false;
                    self.progress = None;
                    self.rx = None;
                    self.mods_notice = Some("Mod 下载已取消".into());
                    self.mods_error = None;
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
        let can_mods = self
            .selected_row()
            .is_some_and(|row| mod_loader(row.loader).is_some());
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
                        } else if self.busy.is_none() && !self.show_install {
                            if can_mods {
                                ui.add_space(4.0);
                                if ghost_button(ui, "下载 Mod").clicked() {
                                    self.open_mods();
                                }
                            }
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
        let width = ui.available_width().min(760.0);
        let pad = ((ui.available_width() - width) / 2.0).max(24.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.label(egui::RichText::new("下载 Mod").size(28.0).strong());
                        if let Some(row) = &self.mods_row {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {} · 下载到该版本的 mods/",
                                    row.mc,
                                    row.loader.name()
                                ))
                                .size(12.0)
                                .color(MUTED),
                            );
                        }
                        ui.add_space(16.0);

                        let mut search = false;
                        card().show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut self.mods_query)
                                        .hint_text("搜索 Mod 名称或描述…")
                                        .desired_width(420.0),
                                );
                                if (response.lost_focus() || response.has_focus())
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                {
                                    search = true;
                                }
                                let mut source = self.mods_source;
                                egui::ComboBox::from_id_salt("mods_source")
                                    .selected_text(source_label(source))
                                    .width(120.0)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_label(source.is_none(), "全部来源")
                                            .clicked()
                                        {
                                            source = None;
                                        }
                                        if ui
                                            .selectable_label(
                                                source == Some(mods::Source::Modrinth),
                                                "Modrinth",
                                            )
                                            .clicked()
                                        {
                                            source = Some(mods::Source::Modrinth);
                                        }
                                        if ui
                                            .selectable_label(
                                                source == Some(mods::Source::CurseForge),
                                                "CurseForge",
                                            )
                                            .clicked()
                                        {
                                            source = Some(mods::Source::CurseForge);
                                        }
                                    });
                                self.mods_source = source;
                                if ui.button("搜索").clicked() {
                                    search = true;
                                }
                            });
                        });
                        if search {
                            self.start_mods_search();
                        }
                        if self.mods_loading {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(egui::RichText::new("正在搜索…").color(MUTED));
                            });
                        }
                        for warning in &self.mods_warnings {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(warning).size(11.5).color(WARN));
                        }
                        if !self.mods_results.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "找到 {} 个结果（合并显示，来源见前缀）",
                                    self.mods_total
                                ))
                                .size(11.5)
                                .color(MUTED),
                            );
                            let mut chosen = None;
                            egui::ScrollArea::vertical()
                                .max_height(235.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for project in &self.mods_results {
                                        let selected =
                                            self.mods_selected.as_ref().is_some_and(|p| {
                                                p.source == project.source && p.id == project.id
                                            });
                                        let text = format!(
                                            "[{}] {}\n{} · {} · {} downloads\n{}",
                                            project.source.label(),
                                            project.name,
                                            project.author,
                                            project.categories.join(", "),
                                            project.downloads,
                                            project.summary
                                        );
                                        if ui.selectable_label(selected, text).clicked() {
                                            chosen = Some(project.clone());
                                        }
                                    }
                                });
                            if let Some(project) = chosen {
                                self.select_mods_project(project);
                            }
                        }

                        let project = self.mods_selected.clone();
                        let Some(project) = project else {
                            if !self.mods_loading && self.mods_results.is_empty() {
                                ui.add_space(16.0);
                                ui.label(
                                    egui::RichText::new(
                                        "输入关键词，搜索 Modrinth 和 CurseForge。",
                                    )
                                    .color(MUTED),
                                );
                            }
                            return;
                        };

                        ui.add_space(16.0);
                        card().show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&project.name).size(19.0).strong());
                                ui.label(
                                    egui::RichText::new(project.source.label())
                                        .size(11.0)
                                        .color(ACCENT),
                                );
                            });
                            ui.label(
                                egui::RichText::new(&project.summary)
                                    .size(12.0)
                                    .color(MUTED),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "作者 {} · {} downloads",
                                    project.author, project.downloads
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                            ui.add_space(12.0);

                            if self.mods_versions.is_empty() {
                                ui.label(
                                    egui::RichText::new("没有匹配当前 Minecraft/加载器的版本")
                                        .color(WARN),
                                );
                            } else {
                                let versions = self.mods_versions.clone();
                                let selected = self.mods_version.clone();
                                let selected_text = selected
                                    .as_ref()
                                    .map(|v| format!("{} · {}", v.version_number, v.file_name))
                                    .unwrap_or_else(|| "选择版本".into());
                                let mut chosen: Option<mods::Version> = None;
                                egui::ComboBox::from_id_salt("mods_version")
                                    .selected_text(selected_text)
                                    .width(500.0)
                                    .show_ui(ui, |ui| {
                                        for version in &versions {
                                            let label = format!(
                                                "{} · {}",
                                                version.version_number, version.file_name
                                            );
                                            if ui
                                                .selectable_label(
                                                    selected
                                                        .as_ref()
                                                        .is_some_and(|v| v.id == version.id),
                                                    label,
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
                                if let Some(version) = &self.mods_version {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "文件: {} · {}",
                                            version.file_name,
                                            version
                                                .size
                                                .map(format_bytes)
                                                .unwrap_or_else(|| "未知大小".into())
                                        ))
                                        .size(11.0)
                                        .color(MUTED),
                                    );
                                }
                            }

                            if self.mods_deps_loading {
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(egui::RichText::new("解析前置依赖…").color(MUTED));
                                });
                            } else if !self.mods_choices.is_empty() {
                                ui.add_space(12.0);
                                ui.label(egui::RichText::new("前置依赖").size(14.0).strong());
                                let choices = self.mods_choices.clone();
                                for choice in &choices {
                                    ui.horizontal(|ui| {
                                        let (label, color) = match choice.dependency.kind {
                                            mods::DependencyKind::Required => ("必需", ACCENT),
                                            mods::DependencyKind::Optional => ("可选", MUTED),
                                            mods::DependencyKind::Incompatible => ("不兼容", WARN),
                                            _ => ("嵌入/工具", MUTED),
                                        };
                                        ui.label(
                                            egui::RichText::new(label).size(11.0).color(color),
                                        );
                                        let name = choice
                                            .project
                                            .as_ref()
                                            .map(|p| p.name.clone())
                                            .or_else(|| choice.dependency.name.clone())
                                            .unwrap_or_else(|| {
                                                choice.dependency.project_id.clone()
                                            });
                                        if choice.dependency.kind == mods::DependencyKind::Optional
                                        {
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
                                        match &choice.version {
                                            Some(version) => {
                                                ui.label(
                                                    egui::RichText::new(&version.file_name)
                                                        .size(11.0)
                                                        .color(MUTED),
                                                );
                                            }
                                            None => {
                                                ui.label(
                                                    egui::RichText::new("找不到兼容版本")
                                                        .size(11.0)
                                                        .color(WARN),
                                                );
                                            }
                                        }
                                    });
                                }
                            }

                            ui.add_space(16.0);
                            let incompatible = self
                                .mods_choices
                                .iter()
                                .any(|c| c.dependency.kind == mods::DependencyKind::Incompatible);
                            let missing_required = self.mods_choices.iter().any(|c| {
                                c.dependency.kind == mods::DependencyKind::Required
                                    && c.version.is_none()
                            });
                            let can_download = self.mods_version.is_some()
                                && !self.mods_deps_loading
                                && !incompatible
                                && !missing_required
                                && self.busy.is_none();
                            if let Some(dir) = &self.mods_game_dir {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "目标: {}",
                                        dir.join("mods").display()
                                    ))
                                    .size(10.5)
                                    .color(MUTED),
                                );
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
                            ui.add_space(10.0);
                            ui.vertical_centered(|ui| {
                                if primary_button(ui, "下载到该版本", can_download) {
                                    self.start_mods_download();
                                }
                            });
                        });
                    });
                });
            });
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
            if is_pack {
                if self.busy.is_none() {
                    self.start_modpack(path);
                } else {
                    self.error = Some("正在安装，请等当前任务结束".into());
                }
            } else {
                self.error = Some(
                    "支持 Modrinth / CurseForge / Prism / HMCL / ATLauncher / MCBBS 整合包".into(),
                );
            }
        }
        if self.busy.is_some() || self.mods_rx.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.top_bar(ui);
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if matches!(self.busy, Some(Task::Install) | Some(Task::Mods)) {
                self.progress_view(ui);
            } else if self.show_mods {
                self.mods_view(ui);
            } else if self.show_manage {
                self.manage_view(ui);
            } else if self.show_install {
                self.install_view(ui);
            } else {
                self.main_view(ui);
            }
        });
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

fn source_label(source: Option<mods::Source>) -> &'static str {
    match source {
        None => "全部来源",
        Some(mods::Source::Modrinth) => "Modrinth",
        Some(mods::Source::CurseForge) => "CurseForge",
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
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
