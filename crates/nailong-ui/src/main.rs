#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::install::{catalog, fabric, forge, health, neoforge, vanilla};
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::{Root, java};
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

#[derive(Clone)]
struct Row {
    id: String,
    label: String,
    mc: String,
    loader: LoaderKind,
    loader_version: Option<String>,
    health: health::Health,
}

#[derive(Clone)]
struct RepairTarget {
    mc: String,
    loader: LoaderKind,
    version: String,
}

impl RepairTarget {
    fn label(&self) -> String {
        if self.loader == LoaderKind::Vanilla {
            self.mc.clone()
        } else {
            format!("{} · {} {}", self.mc, self.loader.name(), self.version)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Task {
    Install,
    Launch,
}

enum Msg {
    Progress(Progress),
    Catalog(Result<catalog::Catalog, String>),
    InstallDone(String),
    InstallFailed(String),
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
        if app.rows.is_empty() {
            app.open_install();
        }
        app
    }

    fn refresh(&mut self) {
        let mut rows = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.root.versions()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().to_string();
                let path = self.root.version_json(&id);
                if !path.exists() {
                    continue;
                }
                let value: serde_json::Value = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                let inherited = value
                    .get("inheritsFrom")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&id)
                    .to_string();

                let (loader, loader_version, mc) =
                    if let Some(rest) = id.strip_prefix("fabric-loader-") {
                        match rest.rsplit_once('-') {
                            Some((version, mc)) => (
                                LoaderKind::Fabric,
                                Some(version.to_string()),
                                mc.to_string(),
                            ),
                            None => (
                                LoaderKind::Fabric,
                                Some(rest.to_string()),
                                inherited.clone(),
                            ),
                        }
                    } else if let Some(rest) = id.strip_prefix("neoforge-") {
                        (
                            LoaderKind::NeoForge,
                            Some(rest.to_string()),
                            inherited.clone(),
                        )
                    } else if let Some(rest) = id.strip_prefix("forge-") {
                        match rest.split_once('-') {
                            Some((mc, version)) => {
                                (LoaderKind::Forge, Some(version.to_string()), mc.to_string())
                            }
                            None => (LoaderKind::Forge, Some(rest.to_string()), inherited.clone()),
                        }
                    } else if let Some((mc, version)) = id.split_once("-forge-") {
                        (LoaderKind::Forge, Some(version.to_string()), mc.to_string())
                    } else {
                        (LoaderKind::Vanilla, None, id.clone())
                    };

                let label = match &loader_version {
                    Some(version) => format!("{mc} · {} {version}", loader.name()),
                    None => mc.clone(),
                };
                let row_health = health::inspect(&self.root, &id);
                rows.push(Row {
                    id,
                    label,
                    mc,
                    loader,
                    loader_version,
                    health: row_health,
                });
            }
        }
        rows.sort_by(|a, b| {
            mc_key(&b.mc)
                .cmp(&mc_key(&a.mc))
                .then_with(|| a.loader.rank().cmp(&b.loader.rank()))
        });
        if self
            .selected
            .as_ref()
            .is_none_or(|s| !rows.iter().any(|r| &r.id == s))
        {
            self.selected = rows.first().map(|r| r.id.clone());
        }
        self.rows = rows;
    }

    fn selected_row(&self) -> Option<Row> {
        self.selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
            .cloned()
    }

    fn open_install(&mut self) {
        self.show_install = true;
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
        self.show_log = false;
        self.busy = Some(Task::Install);
        self.progress = Some(Progress::stage("准备安装"));
        self.progress_label = target_label.clone();
        self.repair_target = Some(RepairTarget {
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
            let result: Result<String, String> = match loader {
                LoaderKind::Vanilla => vanilla::install_with(&root, &mc, "official", &sink)
                    .map(|()| target.clone())
                    .map_err(|e| e.to_string()),
                LoaderKind::Fabric => fabric::install_with(&root, &mc, &version, "official", &sink)
                    .map_err(|e| e.to_string()),
                LoaderKind::NeoForge => {
                    neoforge::install_with(&root, &mc, &version, "official", &sink)
                        .map_err(|e| e.to_string())
                }
                LoaderKind::Forge => forge::install_with(&root, &mc, &version, "official", &sink)
                    .map_err(|e| e.to_string()),
            };
            let msg = match result {
                Ok(id) => Msg::InstallDone(id),
                Err(e) => Msg::InstallFailed(e),
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
                ..Default::default()
            };
            let msg = match launch::launch(&root, &row.id, &opts) {
                Ok(0) => Msg::LaunchDone(0),
                Ok(code) => Msg::LaunchDone(code),
                Err(e) => Msg::LaunchFailed(e.to_string()),
            };
            let _ = tx.send(msg);
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
        for msg in messages {
            match msg {
                Msg::Progress(p) => {
                    if matches!(self.busy, Some(Task::Install)) {
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
                    self.progress = None;
                    self.rx = None;
                    self.show_install = true;
                    self.show_log = true;
                    self.error = Some(e.clone());
                    self.log.push(format!("安装未完成: {e}"));
                    self.refresh();
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
                        if matches!(self.busy, Some(Task::Launch)) {
                            ui.spinner();
                        } else if !self.show_install
                            && !self.rows.is_empty()
                            && self.busy.is_none()
                            && ghost_button(ui, "+ 安装").clicked()
                        {
                            self.open_install();
                        }
                    });
                });
            });
    }

    fn main_view(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self.selected_row() else {
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
                    let rows = self.rows.clone();
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
                                self.start_install_worker(target.mc, target.loader, target.version);
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
                        ui.label(egui::RichText::new("不装加载器").color(MUTED).size(12.0));
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
                        if !self.rows.is_empty() {
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
                ui.label(egui::RichText::new("正在安装").size(26.0).strong());
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
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        if self.busy.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.top_bar(ui);
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if matches!(self.busy, Some(Task::Install)) {
                self.progress_view(ui);
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

fn mc_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
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
            .with_min_inner_size([760.0, 560.0])
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
