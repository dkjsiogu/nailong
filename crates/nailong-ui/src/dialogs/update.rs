//! 启动器自己的更新：启动时安静地查一次（最多 12 小时一次），有新版本时侧栏出现一个小提示，
//! 点开看更新说明、下载（带进度），Windows 一键重启到新版本，Linux 给出安装 deb 的命令。

use crate::prelude::*;
use nailong_core::update::{self, Release};

/// 两次自动检查之间至少隔这么久。
const CHECK_EVERY_SECS: u64 = 12 * 60 * 60;

enum UpdateMsg {
    Checked(Result<Release, String>),
    Progress(Progress),
    Downloaded(Result<PathBuf, String>),
}

#[derive(Default)]
pub(crate) struct UpdateState {
    /// 比现在新的版本；没有新版本或还没查时为 None。
    pub(crate) latest: Option<Release>,
    pub(crate) checking: bool,
    /// 手动检查后给出的结果，例如「已经是最新版」。
    pub(crate) status: Option<String>,
    pub(crate) open: bool,
    rx: Option<mpsc::Receiver<UpdateMsg>>,
    /// 正在下载的进度。
    activity: Option<Activity>,
    downloaded: Option<PathBuf>,
    error: Option<String>,
    started: bool,
}

impl UpdateState {
    /// 侧栏要不要显示「新版本」。
    pub(crate) fn badge(&self, skipped: &str) -> Option<&str> {
        self.latest
            .as_ref()
            .filter(|release| release.version != skipped)
            .map(|release| release.version.as_str())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 更新说明是 Markdown：去掉标题井号和加粗，列表换成圆点，够看就行。
fn plain_notes(notes: &str) -> Vec<String> {
    notes
        .lines()
        .map(|line| {
            let line = line.trim_end().replace("**", "");
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed
                .strip_prefix("* ")
                .or_else(|| trimmed.strip_prefix("- "))
            {
                format!("· {rest}")
            } else {
                trimmed.trim_start_matches('#').trim_start().to_string()
            }
        })
        .filter(|line| !line.contains("Co-Authored-By"))
        .collect()
}

impl App {
    /// 每帧调用：第一次时清理上次更新留下的旧程序，并按需在后台检查；收后台结果。
    pub(crate) fn poll_update(&mut self, ctx: &egui::Context) {
        if !self.update.started {
            self.update.started = true;
            update::cleanup();
            if now().saturating_sub(self.prefs.update_checked) > CHECK_EVERY_SECS {
                self.check_update(ctx, false);
            }
        }
        let Some(rx) = &self.update.rx else {
            return;
        };
        let mut done = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                UpdateMsg::Checked(result) => {
                    self.update.checking = false;
                    self.prefs.update_checked = now();
                    self.save_ui_config();
                    done = true;
                    match result {
                        Ok(release) if update::is_newer(&release.version, update::current()) => {
                            self.update.status = None;
                            self.update.latest = Some(release);
                        }
                        Ok(_) => {
                            self.update.latest = None;
                            self.update.status =
                                Some(format!("已经是最新版 {}", update::current()));
                        }
                        Err(e) => self.update.status = Some(format!("没查到：{e}")),
                    }
                }
                UpdateMsg::Progress(p) => {
                    if let Some(activity) = &mut self.update.activity {
                        activity.update(p);
                    }
                }
                UpdateMsg::Downloaded(result) => {
                    done = true;
                    self.update.activity = None;
                    match result {
                        Ok(path) => self.update.downloaded = Some(path),
                        Err(e) => self.update.error = Some(e),
                    }
                }
            }
        }
        if done {
            self.update.rx = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// 在后台查 GitHub 上的最新版本。`manual` 时结果写在设置里。
    pub(crate) fn check_update(&mut self, ctx: &egui::Context, manual: bool) {
        if self.update.rx.is_some() {
            return;
        }
        self.update.checking = true;
        if manual {
            self.update.status = Some("正在检查…".into());
        }
        let (tx, rx) = mpsc::channel();
        self.update.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = nailong_core::http::Http::new("official")
                .and_then(|http| update::latest(&http))
                .map_err(|e| e.to_string());
            let _ = tx.send(UpdateMsg::Checked(result));
            ctx.request_repaint();
        });
    }

    fn start_update_download(&mut self, ctx: &egui::Context) {
        let Some(release) = self.update.latest.clone() else {
            return;
        };
        let Some(asset) = update::asset_for_this_system(&release).cloned() else {
            self.update.error = Some("这个系统没有现成的安装包，请到 GitHub 页面下载".into());
            return;
        };
        self.update.error = None;
        self.update.downloaded = None;
        self.update.activity = Some(Activity::new(
            format!("Cairn {}", release.version),
            "准备下载",
        ));
        let dir = update::download_dir(&self.root.cache());
        let (tx, rx) = mpsc::channel();
        self.update.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let progress = tx.clone();
            let repaint = ctx.clone();
            let sink = from_fn(move |p| {
                let _ = progress.send(UpdateMsg::Progress(p));
                repaint.request_repaint();
            });
            let result = nailong_core::http::Http::new("official")
                .and_then(|http| update::download(&http, &release, &asset, &dir, &sink))
                .map_err(|e| e.to_string());
            let _ = tx.send(UpdateMsg::Downloaded(result));
            ctx.request_repaint();
        });
    }

    pub(crate) fn update_overlay(&mut self, ctx: &egui::Context) {
        if !self.update.open {
            return;
        }
        let Some(release) = self.update.latest.clone() else {
            self.update.open = false;
            return;
        };
        let mut close = false;
        let mut skip = false;
        let mut download = false;
        let mut restart = false;
        let mut open_page = false;
        let mut reveal: Option<PathBuf> = None;
        let mut copy: Option<String> = None;
        let windows = cfg!(windows);
        let modal = egui::Modal::new(egui::Id::unique("update_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(24, 20)),
            )
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(
                    egui::RichText::new(format!("Cairn {}", release.version))
                        .size(24.0)
                        .family(display_family()),
                );
                ui.label(
                    egui::RichText::new(format!("现在是 {}", update::current()))
                        .size(13.0)
                        .color(t::sub()),
                );
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(t::bg())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let lines = plain_notes(&release.notes);
                                if lines.iter().all(|l| l.trim().is_empty()) {
                                    ui.label(
                                        egui::RichText::new("这一版没有写更新说明")
                                            .size(13.0)
                                            .color(t::dim()),
                                    );
                                }
                                for line in lines {
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(line).size(13.0))
                                            .wrap(),
                                    );
                                }
                            });
                    });
                ui.add_space(12.0);
                if let Some(activity) = &self.update.activity {
                    let p = &activity.progress;
                    motion::progress_bar(ui, p.fraction(), p.indeterminate, ui.input(|i| i.time));
                    ui.add_space(6.0);
                    let mut parts = vec![p.stage.clone()];
                    if p.bytes_total > 0 {
                        parts.push(progress_detail(p));
                    }
                    if let Some(speed) = activity.speed() {
                        parts.push(rate(speed));
                    }
                    if let Some(source) = activity.source_label() {
                        parts.push(format!("来自 {source}"));
                    }
                    ui.label(
                        egui::RichText::new(parts.join(" · "))
                            .size(13.0)
                            .color(t::sub()),
                    );
                } else if let Some(path) = &self.update.downloaded {
                    if windows {
                        ui.label(
                            egui::RichText::new(
                                "下载好了，校验通过。重启后就是新版本，数据和设置都不变。",
                            )
                            .size(13.0)
                            .color(t::ok()),
                        );
                    } else {
                        let command = update::install_command(path);
                        ui.label(
                            egui::RichText::new("下载好了，校验通过。在终端里运行这条命令安装：")
                                .size(13.0)
                                .color(t::ok()),
                        );
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(&command).monospace().size(12.5));
                        ui.horizontal(|ui| {
                            if action_link(ui, "复制命令").clicked() {
                                copy = Some(command.clone());
                            }
                            if action_link(ui, "在文件夹中显示").clicked() {
                                reveal = Some(path.clone());
                            }
                        });
                    }
                }
                if let Some(error) = &self.update.error {
                    ui.label(egui::RichText::new(error).size(13.0).color(t::danger()));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if action_link(ui, "在 GitHub 上看").clicked() {
                        open_page = true;
                    }
                    if action_link(ui, "跳过这个版本").clicked() {
                        skip = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let busy = self.update.activity.is_some();
                        match (&self.update.downloaded, windows) {
                            (Some(_), true) => {
                                if motion::primary_sized(
                                    ui,
                                    "重启到新版本",
                                    true,
                                    120.0,
                                    32.0,
                                    12.0,
                                ) {
                                    restart = true;
                                }
                            }
                            (Some(_), false) => {}
                            (None, _) => {
                                let label = if windows {
                                    "下载并更新"
                                } else {
                                    "下载 deb"
                                };
                                if motion::primary_sized(ui, label, !busy, 110.0, 32.0, 12.0) {
                                    download = true;
                                }
                            }
                        }
                        ui.add_space(6.0);
                        if motion::secondary_sized(ui, "以后再说", true, 80.0, 32.0, 12.0).clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if let Some(text) = copy {
            ctx.copy_text(text);
        }
        if let Some(path) = reveal {
            reveal_file(&path);
        }
        if open_page {
            open_url(&release.page);
        }
        if download {
            self.start_update_download(ctx);
        }
        if restart && let Some(path) = self.update.downloaded.clone() {
            self.save_ui_config();
            match update::replace_running_exe(&path) {
                Ok(()) => std::process::exit(0),
                Err(e) => self.update.error = Some(format!("没能换到新版本：{e}")),
            }
        }
        if skip {
            self.prefs.update_skip = release.version.clone();
            self.save_ui_config();
            close = true;
        }
        if close {
            self.update.open = false;
        }
    }
}

/// 侧栏底部的「新版本」提示；返回是否被点了。
pub(crate) fn update_pill(ui: &mut egui::Ui, version: &str, wide: bool) -> bool {
    let text = if wide {
        format!("新版本 {version}")
    } else {
        "新".to_string()
    };
    let response = ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::accent()))
            .fill(t::tint(t::accent(), 0.10))
            .stroke(egui::Stroke::new(1.0, t::tint(t::accent(), 0.45)))
            .corner_radius(motion::R)
            .min_size(egui::vec2(ui.available_width(), 30.0)),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
        .on_hover_text(format!("Cairn {version} 可以更新了，点开看看改了什么"))
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_notes_read_as_plain_text() {
        let notes = "## 改了什么\n* **启动参数**看得见\n- 截图墙\n\nCo-Authored-By: someone";
        assert_eq!(
            plain_notes(notes),
            ["改了什么", "· 启动参数看得见", "· 截图墙", ""]
        );
    }

    #[test]
    fn skipped_versions_have_no_badge() {
        let state = UpdateState {
            latest: Some(Release {
                version: "9.9.9".into(),
                notes: String::new(),
                page: String::new(),
                assets: Vec::new(),
            }),
            ..Default::default()
        };
        assert_eq!(state.badge(""), Some("9.9.9"));
        assert_eq!(state.badge("9.9.9"), None);
    }
}
