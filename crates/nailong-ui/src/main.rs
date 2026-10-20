#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::install::{catalog, health, neoforge, vanilla};
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::{Root, java};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(242, 193, 78);
const CARD: egui::Color32 = egui::Color32::from_rgb(24, 27, 33);
const MUTED: egui::Color32 = egui::Color32::from_rgb(145, 148, 156);
const WARN: egui::Color32 = egui::Color32::from_rgb(242, 160, 90);

#[derive(Clone)]
struct Row {
    id: String,
    label: String,
    mc: String,
    nf: Option<String>,
    health: health::Health,
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
    install_nf: String,
    repair_mc: Option<String>,
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
            install_nf: "21.1.209".into(),
            repair_mc: None,
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
                let mc = value
                    .get("inheritsFrom")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&id)
                    .to_string();
                let nf = id.strip_prefix("neoforge-").map(str::to_string);
                let label = match &nf {
                    Some(nf) => format!("{mc} · NeoForge {nf}"),
                    None => id.clone(),
                };
                let row_health = health::inspect(&self.root, &id);
                rows.push(Row {
                    id,
                    label,
                    mc,
                    nf,
                    health: row_health,
                });
            }
        }
        rows.sort_by(|a, b| {
            b.nf.is_some()
                .cmp(&a.nf.is_some())
                .then_with(|| b.id.cmp(&a.id))
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
        self.repair_mc = None;
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
        let Some(cat) = &self.catalog else {
            return;
        };
        let mc = if cat.minecraft.iter().any(|v| v == "1.21.1") {
            "1.21.1".to_string()
        } else {
            cat.minecraft.first().cloned().unwrap_or_default()
        };
        self.install_mc = mc.clone();
        self.install_nf = preferred_nf(cat, &mc);
    }

    fn start_install(&mut self, mc: String, nf: String) {
        if self.busy.is_some() {
            return;
        }
        if nf.is_empty() {
            self.error = Some("这个 Minecraft 版本没有可用的 NeoForge".into());
            return;
        }
        self.start_install_worker(mc, Some(nf));
    }

    fn start_repair(&mut self, row: Row) {
        if self.busy.is_some() {
            return;
        }
        self.start_install_worker(row.mc, row.nf);
    }

    fn start_install_worker(&mut self, mc: String, nf: Option<String>) {
        self.error = None;
        self.notice = None;
        self.show_install = true;
        self.show_log = false;
        self.busy = Some(Task::Install);
        self.progress = Some(Progress::stage("准备安装"));
        self.progress_label = match &nf {
            Some(nf) => format!("{mc} · NeoForge {nf}"),
            None => mc.clone(),
        };
        self.repair_mc = nf.is_none().then(|| mc.clone());
        self.install_nf = nf.clone().unwrap_or_default();
        let target = match &nf {
            Some(nf) => {
                self.log.push(format!("安装 {mc} + NeoForge {nf}"));
                format!("neoforge-{nf}")
            }
            None => {
                self.log.push(format!("补全 {mc} 的资源"));
                mc.clone()
            }
        };
        self.install_mc = mc.clone();
        if let Some(nf) = &nf {
            self.install_nf = nf.clone();
        }

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
            let result = match &nf {
                Some(nf) => neoforge::install_with(&root, &mc, nf, "official", &sink),
                None => vanilla::install_with(&root, &mc, "official", &sink),
            };
            let msg = match result {
                Ok(()) => Msg::InstallDone(target),
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
                        Ok(cat) => {
                            self.catalog = Some(cat);
                        }
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
                    self.repair_mc = None;
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
        egui::Panel::top("top").default_size(60.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("nailong")
                        .size(23.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(18.0);
                    if matches!(self.busy, Some(Task::Launch)) {
                        ui.spinner();
                    } else if !self.show_install
                        && !self.rows.is_empty()
                        && self.busy.is_none()
                        && ui.button("+ 安装").clicked()
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
        let width = ui.available_width().min(560.0);
        let pad = ((ui.available_width() - width) / 2.0).max(20.0);
        ui.add_space(30.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);

                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("版本").color(MUTED));
                    let mut chosen = row.id.clone();
                    let rows = self.rows.clone();
                    egui::ComboBox::from_id_salt("installed_version")
                        .selected_text(row.label.clone())
                        .width(330.0)
                        .show_ui(ui, |ui| {
                            for r in &rows {
                                ui.selectable_value(&mut chosen, r.id.clone(), r.label.clone());
                            }
                        });
                    if chosen != row.id {
                        self.selected = Some(chosen);
                    }
                });

                ui.add_space(22.0);
                if row.health.missing > 0 {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "资源不完整，缺少 {} 个文件",
                                    row.health.missing
                                ))
                                .size(15.0)
                                .color(WARN),
                            );
                            ui.add_space(10.0);
                            let mut repair = false;
                            if primary_button(ui, "补全资源", true) {
                                repair = true;
                            }
                            if repair {
                                self.start_repair(row.clone());
                            }
                        });
                } else {
                    let launching = matches!(self.busy, Some(Task::Launch));
                    if launching {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("正在启动游戏…").size(15.0));
                        });
                    } else if primary_button(ui, "启动游戏", self.busy.is_none()) {
                        self.start_launch();
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

                ui.add_space(26.0);
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("玩家名").color(MUTED));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.username).desired_width(160.0),
                            );
                            ui.add_space(22.0);
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

                ui.add_space(16.0);
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
        let width = ui.available_width().min(560.0);
        let pad = ((ui.available_width() - width) / 2.0).max(20.0);
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                let title = if self.error.is_some() {
                    "安装未完成"
                } else {
                    "安装游戏"
                };
                ui.label(egui::RichText::new(title).size(26.0).strong());
                ui.add_space(16.0);

                let catalog = self
                    .catalog
                    .clone()
                    .unwrap_or_else(catalog::Catalog::fallback);
                let mc_options = catalog.minecraft.clone();
                let mut mc = self.install_mc.clone();
                let mut nf = self.install_nf.clone();

                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized([88.0, 22.0], egui::Label::new("Minecraft"));
                            egui::ComboBox::from_id_salt("install_mc")
                                .selected_text(mc.clone())
                                .width(260.0)
                                .show_ui(ui, |ui| {
                                    for v in &mc_options {
                                        ui.selectable_value(&mut mc, v.clone(), v);
                                    }
                                });
                        });
                        if mc != self.install_mc {
                            self.install_mc = mc.clone();
                            self.install_nf = preferred_nf(&catalog, &mc);
                            nf = self.install_nf.clone();
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.add_sized([88.0, 22.0], egui::Label::new("NeoForge"));
                            let nf_options = nf_options(&catalog, &mc);
                            egui::ComboBox::from_id_salt("install_nf")
                                .selected_text(if nf.is_empty() {
                                    "—".to_string()
                                } else {
                                    nf.clone()
                                })
                                .width(260.0)
                                .show_ui(ui, |ui| {
                                    for v in &nf_options {
                                        ui.selectable_value(&mut nf, v.clone(), v);
                                    }
                                });
                            if nf != self.install_nf {
                                self.install_nf = nf.clone();
                            }
                        });

                        if self.catalog_loading {
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(egui::RichText::new("获取版本…").color(MUTED));
                            });
                        } else if let Some(err) = &self.catalog_error {
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new(err).size(11.5).color(MUTED));
                        }

                        ui.add_space(18.0);
                        let options = nf_options(&catalog, &mc);
                        let can_install = self.busy.is_none()
                            && !mc.is_empty()
                            && (self.repair_mc.is_some() || options.contains(&nf));
                        let action = if self.error.is_some() {
                            "继续安装"
                        } else {
                            "安装"
                        };
                        let mut go = false;
                        if primary_button(ui, action, can_install) {
                            go = true;
                        }
                        if !self.rows.is_empty() {
                            ui.add_space(8.0);
                            if ui.button("返回").clicked() {
                                self.show_install = false;
                                self.error = None;
                            }
                        }
                        if go {
                            self.install_mc = mc.clone();
                            self.install_nf = nf.clone();
                            if let Some(repair) = self.repair_mc.clone() {
                                self.start_install_worker(repair, None);
                            } else {
                                self.start_install(mc, nf);
                            }
                        }
                    });

                if !self.java_ok {
                    java_warning(ui);
                }
                if let Some(err) = &self.error {
                    ui.add_space(12.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(err).color(egui::Color32::from_rgb(230, 120, 110)),
                        )
                        .wrap(),
                    );
                }
            });
        });
    }

    fn progress_view(&mut self, ui: &mut egui::Ui) {
        let p = self
            .progress
            .clone()
            .unwrap_or_else(|| Progress::stage("准备安装"));
        let width = ui.available_width().min(560.0);
        let pad = ((ui.available_width() - width) / 2.0).max(20.0);
        ui.add_space(40.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.label(egui::RichText::new("正在安装").size(26.0).strong());
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&self.progress_label).color(MUTED));
                ui.add_space(24.0);
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
                            .desired_width(420.0)
                            .desired_height(12.0)
                            .fill(ACCENT)
                            .animate(true),
                    );
                } else {
                    ui.add(
                        egui::ProgressBar::new(p.fraction())
                            .desired_width(420.0)
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

fn java_warning(ui: &mut egui::Ui) {
    ui.add_space(10.0);
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
    .min_size(egui::vec2(180.0, 44.0));
    ui.add_enabled(enabled, button).clicked()
}

fn preferred_nf(cat: &catalog::Catalog, mc: &str) -> String {
    let list = cat.neoforge_for(mc);
    if mc == "1.21.1" && list.iter().any(|v| v == "21.1.209") {
        return "21.1.209".to_string();
    }
    list.first().cloned().unwrap_or_default()
}

fn nf_options(cat: &catalog::Catalog, mc: &str) -> Vec<String> {
    let mut list = cat.neoforge_for(mc).to_vec();
    if mc == "1.21.1"
        && let Some(pos) = list.iter().position(|v| v == "21.1.209")
    {
        let v = list.remove(pos);
        list.insert(0, v);
    }
    list
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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([860.0, 560.0])
            .with_min_inner_size([700.0, 480.0])
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
