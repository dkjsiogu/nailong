#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui;
use nailong_core::Root;
use nailong_core::install::neoforge;
use nailong_core::launch::{self, LaunchOptions};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(242, 193, 78);

#[derive(Clone)]
struct Row {
    id: String,
    loader: String,
    java: String,
    path: String,
}

enum Msg {
    Status(String),
    Done(String),
    Error(String),
}

struct App {
    root: Root,
    rows: Vec<Row>,
    selected: Option<String>,
    username: String,
    memory: u32,
    install_mc: String,
    install_nf: String,
    busy: bool,
    status: String,
    log: Vec<String>,
    rx: Option<mpsc::Receiver<Msg>>,
}

impl App {
    fn new(root: Root) -> Self {
        let mut app = Self {
            root,
            rows: Vec::new(),
            selected: None,
            username: "Player".into(),
            memory: 4096,
            install_mc: "1.21.1".into(),
            install_nf: "21.1.209".into(),
            busy: false,
            status: "就绪".into(),
            log: vec![
                "就绪。选择实例后点击“启动”。".into(),
                "第一次使用：展开“安装 NeoForge”，填写版本后开始。".into(),
            ],
            rx: None,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        let mut rows = Vec::new();
        let dir = self.root.versions();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                if !e.path().is_dir() {
                    continue;
                }
                let id = e.file_name().to_string_lossy().to_string();
                let json_path = self.root.version_json(&id);
                if !json_path.exists() {
                    continue;
                }
                let value: serde_json::Value = std::fs::read(&json_path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                let main = value
                    .get("mainClass")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let loader = if id.starts_with("neoforge-") || id.contains("-neoforge-") {
                    "NeoForge".to_string()
                } else if id.contains("forge") {
                    "Forge".to_string()
                } else if main.contains("fabric") {
                    "Fabric".to_string()
                } else if main.contains("quilt") {
                    "Quilt".to_string()
                } else {
                    "原版".to_string()
                };
                let java = value
                    .get("javaVersion")
                    .and_then(|v| v.get("majorVersion"))
                    .and_then(|v| v.as_u64())
                    .map(|v| format!("Java {v}"))
                    .unwrap_or_else(|| "Java 自动".into());
                rows.push(Row {
                    id,
                    loader,
                    java,
                    path: json_path.display().to_string(),
                });
            }
        }
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        if self
            .selected
            .as_ref()
            .is_none_or(|s| !rows.iter().any(|r| &r.id == s))
        {
            self.selected = rows.first().map(|r| r.id.clone());
        }
        self.rows = rows;
    }

    fn selected_row(&self) -> Option<&Row> {
        self.selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
    }

    fn push_log(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 300 {
            self.log.remove(0);
        }
    }

    fn start_install(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "正在安装 NeoForge…".into();
        self.push_log(format!(
            "开始安装 NeoForge {}（Minecraft {}）",
            self.install_nf, self.install_mc
        ));
        let root = self.root.clone();
        let mc = self.install_mc.clone();
        let nf = self.install_nf.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Status("下载并安装原版 / NeoForge…".into()));
            match neoforge::install(&root, &mc, &nf, "official") {
                Ok(()) => {
                    let _ = tx.send(Msg::Done(format!("NeoForge {nf} 安装完成")));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("安装失败：{e}")));
                }
            }
        });
    }

    fn start_launch(&mut self) {
        if self.busy {
            return;
        }
        let Some(row) = self.selected.clone() else {
            return;
        };
        self.busy = true;
        self.status = "正在启动…".into();
        self.push_log(format!("启动 {}", row));
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
            match launch::launch(&root, &row, &opts) {
                Ok(0) => {
                    let _ = tx.send(Msg::Done("游戏正常退出".into()));
                }
                Ok(code) => {
                    let _ = tx.send(Msg::Error(format!("游戏异常退出（exit {code}）")));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error(format!("启动失败：{e}")));
                }
            }
        });
    }

    fn poll(&mut self) {
        let mut done = None;
        let messages: Vec<Msg> = if let Some(rx) = &self.rx {
            let mut v = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                v.push(msg);
            }
            v
        } else {
            Vec::new()
        };
        for msg in messages {
            match msg {
                Msg::Status(s) => {
                    self.status = s.clone();
                    self.push_log(s);
                }
                Msg::Done(s) => {
                    self.status = s.clone();
                    self.push_log(s);
                    done = Some(true);
                }
                Msg::Error(s) => {
                    self.status = s.clone();
                    self.push_log(s);
                    done = Some(false);
                }
            }
        }
        if done.is_some() {
            self.busy = false;
            self.rx = None;
            self.refresh();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        if self.busy {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }

        egui::Panel::top("top").default_size(64.0).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new("nailong")
                        .size(24.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.label(
                    egui::RichText::new("轻量 Minecraft 启动器")
                        .size(13.0)
                        .color(egui::Color32::from_gray(150)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(18.0);
                    if self.busy {
                        ui.spinner();
                    }
                    ui.label(
                        egui::RichText::new(&self.status)
                            .size(13.0)
                            .color(egui::Color32::from_gray(190)),
                    );
                });
            });
        });

        egui::Panel::left("side")
            .resizable(false)
            .default_size(280.0)
            .show(ui, |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.heading("实例");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("刷新").clicked() {
                            self.refresh();
                        }
                    });
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.rows.is_empty() {
                            ui.add_space(24.0);
                            ui.label(
                                egui::RichText::new("还没有安装版本")
                                    .color(egui::Color32::from_gray(150)),
                            );
                            ui.label(
                                egui::RichText::new("在右侧安装 NeoForge")
                                    .color(egui::Color32::from_gray(120)),
                            );
                        }
                        for row in &self.rows {
                            let selected = self.selected.as_ref() == Some(&row.id);
                            let text = format!("{}\n{} · {}", row.id, row.loader, row.java);
                            if ui
                                .add_sized([250.0, 56.0], egui::Button::selectable(selected, text))
                                .clicked()
                            {
                                self.selected = Some(row.id.clone());
                            }
                        }
                    });
            });

        egui::CentralPanel::default_margins().show(ui, |ui| {
            ui.add_space(18.0);
            if let Some(row) = self.selected_row().cloned() {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&row.id).size(28.0).strong());
                        ui.label(egui::RichText::new(format!("{} · {}", row.loader, row.java)).size(14.0).color(egui::Color32::from_gray(160)));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = egui::Button::new(egui::RichText::new("启动").size(17.0).strong().color(egui::Color32::BLACK))
                            .fill(ACCENT)
                            .min_size(egui::vec2(120.0, 42.0));
                        if ui.add_enabled(!self.busy, btn).clicked() { self.start_launch(); }
                    });
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new(&row.path).size(11.0).monospace().color(egui::Color32::from_gray(120)));
                ui.add_space(18.0);

                egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("离线用户名");
                        ui.add(egui::TextEdit::singleline(&mut self.username).desired_width(180.0));
                        ui.add_space(16.0);
                        ui.label("内存");
                        ui.add(egui::DragValue::new(&mut self.memory).range(1024..=16384).suffix(" MB"));
                    });
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("离线模式：用于单人/局域网；不绕过正版服务器验证。").size(11.0).color(egui::Color32::from_gray(130)));
                });

                ui.add_space(16.0);
                egui::CollapsingHeader::new(egui::RichText::new("安装 NeoForge").size(16.0).strong()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Minecraft");
                        ui.add(egui::TextEdit::singleline(&mut self.install_mc).desired_width(90.0));
                        ui.add_space(12.0);
                        ui.label("NeoForge");
                        ui.add(egui::TextEdit::singleline(&mut self.install_nf).desired_width(110.0));
                        ui.add_space(12.0);
                        if ui.add_enabled(!self.busy, egui::Button::new("开始安装")).clicked() {
                            self.start_install();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("首次安装会下载原版资源（约 1 GB），并使用 NeoForge 官方安装器完成一次性安装；之后可离线启动。").size(11.0).color(egui::Color32::from_gray(130)));
                });

                ui.add_space(18.0);
                ui.label(egui::RichText::new("日志").size(16.0).strong());
                egui::ScrollArea::vertical().max_height(190.0).auto_shrink([false, false]).show(ui, |ui| {
                    for line in &self.log {
                        ui.label(egui::RichText::new(line).monospace().size(11.5).color(egui::Color32::from_gray(180)));
                    }
                });
            } else {
                ui.add_space(60.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("还没有版本").size(26.0).strong());
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("展开下面的“安装 NeoForge”，填写版本后开始。").color(egui::Color32::from_gray(150)));
                    ui.add_space(20.0);
                    egui::CollapsingHeader::new(egui::RichText::new("安装 NeoForge").size(16.0).strong()).default_open(true).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Minecraft");
                            ui.add(egui::TextEdit::singleline(&mut self.install_mc).desired_width(90.0));
                            ui.label("NeoForge");
                            ui.add(egui::TextEdit::singleline(&mut self.install_nf).desired_width(110.0));
                            if ui.add_enabled(!self.busy, egui::Button::new("开始安装")).clicked() { self.start_install(); }
                        });
                    });
                });
            }
        });
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
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("nailong"),
        ..Default::default()
    };
    eframe::run_native(
        "nailong",
        options,
        Box::new(move |cc| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = egui::Color32::from_rgb(18, 20, 24);
            visuals.window_fill = egui::Color32::from_rgb(24, 27, 33);
            visuals.faint_bg_color = egui::Color32::from_rgb(30, 34, 41);
            visuals.selection.bg_fill = ACCENT;
            visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
            cc.egui_ctx.set_visuals(visuals);
            install_cjk(&cc.egui_ctx);
            Ok(Box::new(App::new(root)))
        }),
    )
}
