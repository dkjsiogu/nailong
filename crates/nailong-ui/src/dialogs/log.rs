//! 日志窗口。启动失败时一步打开这次的游戏输出，错误行标红，能认出的原因放在最上面；
//! 不用再去设置 › 数据目录 › cache › logs 里翻文件。

use crate::prelude::*;
use crate::{motion, theme as t};
use eframe::egui;
use nailong_core::launch;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 一次最多读日志末尾这么多，几十 MB 的 debug 日志也不会卡住界面。
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Game,
    Crash,
    Launcher,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Running,
    Exited(i32),
    /// 游戏进程没起来，原因在启动器自己的记录里。
    Failed(String),
}

/// 最近一次启动：日志在哪、什么时候开始、怎么结束的。
#[derive(Clone, Debug)]
pub struct LastLaunch {
    pub label: String,
    pub log: PathBuf,
    pub game_dir: PathBuf,
    pub started: SystemTime,
    pub outcome: Outcome,
}

impl LastLaunch {
    /// 这次启动之后写下的崩溃报告：游戏的 crash-reports，或者 JVM 自己的 hs_err。
    pub fn crash_report(&self) -> Option<PathBuf> {
        let since = self
            .started
            .checked_sub(std::time::Duration::from_secs(2))
            .unwrap_or(UNIX_EPOCH);
        let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
        for (dir, prefix, ext) in [
            (self.game_dir.join("crash-reports"), "crash-", "txt"),
            (self.game_dir.clone(), "hs_err_pid", "log"),
        ] {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with(prefix) || !name.ends_with(ext) {
                    continue;
                }
                if let Some(modified) = entry.metadata().ok().and_then(|m| m.modified().ok())
                    && modified >= since
                {
                    found.push((modified, entry.path()));
                }
            }
        }
        found
            .into_iter()
            .max_by_key(|(when, _)| *when)
            .map(|(_, path)| path)
    }
}

/// 读文件末尾，最多 `TAIL_BYTES`；截断时丢掉开头那半行。
fn read_tail(path: &Path) -> Option<(String, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0
        && let Some(cut) = text.find('\n')
    {
        text.drain(..=cut);
    }
    Some((text, len))
}

fn is_warn_line(line: &str) -> bool {
    line.contains("/WARN]") || line.contains("[WARN]") || line.contains(" WARN ")
}

pub struct LogView {
    pub tab: Tab,
    errors_only: bool,
    query: String,
    /// 现在显示的文件和读到的长度；启动器标签为 None。
    source: Option<PathBuf>,
    len: u64,
    lines: Vec<String>,
    errors: usize,
    diagnosis: Option<String>,
    last_error: Option<String>,
    /// 筛选后显示哪些行，和算它用的条件。
    shown: Vec<usize>,
    shown_key: Option<(bool, String, usize)>,
    checked_at: f64,
}

impl LogView {
    pub fn new(tab: Tab) -> Self {
        Self {
            tab,
            errors_only: false,
            query: String::new(),
            source: None,
            len: 0,
            lines: Vec::new(),
            errors: 0,
            diagnosis: None,
            last_error: None,
            shown: Vec::new(),
            shown_key: None,
            checked_at: f64::NEG_INFINITY,
        }
    }

    fn load_text(&mut self, text: &str, len: u64) {
        self.lines = text
            .lines()
            .map(|line| line.replace('\t', "    "))
            .collect();
        self.len = len;
        self.errors = self
            .lines
            .iter()
            .filter(|line| launch::is_error_line(line))
            .count();
        self.diagnosis = launch::diagnose(text);
        self.last_error = launch::error_lines(text, 1).pop();
        self.shown_key = None;
    }

    /// 切换标签或文件变长时重新读。
    fn sync(&mut self, path: Option<PathBuf>, launcher: &[String], now: f64, live: bool) {
        match path {
            None => {
                if self.source.is_some() || self.lines.len() != launcher.len() {
                    self.source = None;
                    let text = launcher.join("\n");
                    self.load_text(&text, text.len() as u64);
                }
            }
            Some(path) => {
                let switched = self.source.as_deref() != Some(path.as_path());
                let due = live && now - self.checked_at > 1.0;
                if switched || due {
                    self.checked_at = now;
                    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    if switched || len != self.len {
                        match read_tail(&path) {
                            Some((text, len)) => self.load_text(&text, len),
                            None => self.load_text("", 0),
                        }
                    }
                    self.source = Some(path);
                }
            }
        }
        let query = self.query.trim().to_lowercase();
        let key = (self.errors_only, query.clone(), self.lines.len());
        if self.shown_key.as_ref() != Some(&key) {
            self.shown = self
                .lines
                .iter()
                .enumerate()
                .filter(|(_, line)| !self.errors_only || launch::is_error_line(line))
                .filter(|(_, line)| query.is_empty() || line.to_lowercase().contains(&query))
                .map(|(index, _)| index)
                .collect();
            self.shown_key = Some(key);
        }
    }

    /// 画日志窗口；返回 true 表示关掉。
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        last: Option<&LastLaunch>,
        launcher: &[String],
    ) -> bool {
        let crash = last.and_then(LastLaunch::crash_report);
        if self.tab == Tab::Crash && crash.is_none() {
            self.tab = Tab::Game;
        }
        if self.tab == Tab::Game && last.is_none() {
            self.tab = Tab::Launcher;
        }
        let path = match self.tab {
            Tab::Game => last.map(|last| last.log.clone()),
            Tab::Crash => crash.clone(),
            Tab::Launcher => None,
        };
        let running = last.is_some_and(|last| matches!(last.outcome, Outcome::Running));
        let live = running && self.tab == Tab::Game;
        let now = ctx.input(|i| i.time);
        self.sync(path.clone(), launcher, now, live);
        if live {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }

        let mut close = false;
        let mut wanted_tab = None;
        let screen = ctx.content_rect();
        let width = (screen.width() - 96.0).clamp(520.0, 860.0);
        let body_h = (screen.height() - 330.0).clamp(160.0, 460.0);
        let modal = egui::Modal::new(egui::Id::unique("log_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(24, 20)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("日志")
                                .size(24.0)
                                .family(display_family()),
                        );
                        let subtitle = match (self.tab, last) {
                            (Tab::Launcher, _) | (_, None) => {
                                "启动器做过的事，最近的在最下面".to_string()
                            }
                            (_, Some(last)) => {
                                let outcome = match &last.outcome {
                                    Outcome::Running => "运行中".to_string(),
                                    Outcome::Exited(0) => "正常退出".to_string(),
                                    Outcome::Exited(code) => format!("异常退出（代码 {code}）"),
                                    Outcome::Failed(reason) => {
                                        format!("没能启动：{}", short_text(reason, 30))
                                    }
                                };
                                let when = last
                                    .started
                                    .duration_since(UNIX_EPOCH)
                                    .map(|d| human_age(d.as_secs()))
                                    .unwrap_or_default();
                                format!("{} · {outcome} · {when}", short_text(&last.label, 28))
                            }
                        };
                        ui.label(egui::RichText::new(subtitle).size(13.0).color(t::sub()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("×").size(18.0)).frame(false),
                            )
                            .on_hover_text("关闭（Esc）")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });

                // 认得出的原因放最上面；认不出时给最后一个错误。
                let failed = last
                    .is_some_and(|last| matches!(last.outcome, Outcome::Exited(code) if code != 0));
                let headline = match (&self.diagnosis, &self.last_error) {
                    (Some(reason), _) => Some(("可能的原因", reason.clone())),
                    (None, Some(line)) if failed || self.tab == Tab::Crash => {
                        Some(("最后一个错误", line.clone()))
                    }
                    _ => None,
                };
                if let Some((label, text)) = headline
                    && self.tab != Tab::Launcher
                {
                    ui.add_space(12.0);
                    egui::Frame::new()
                        .fill(t::tint(t::warn(), 0.12))
                        .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.45)))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(label)
                                        .size(13.0)
                                        .family(bold_family())
                                        .color(t::warn()),
                                );
                                ui.add(
                                    egui::Label::new(egui::RichText::new(text).size(13.0))
                                        .truncate(),
                                );
                            });
                        });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let mut tabs = vec![(Tab::Game, "游戏输出")];
                    if crash.is_some() {
                        tabs.push((Tab::Crash, "崩溃报告"));
                    }
                    tabs.push((Tab::Launcher, "启动器"));
                    for (tab, label) in tabs {
                        if chip(ui, label, self.tab == tab).clicked() && self.tab != tab {
                            wanted_tab = Some(tab);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = format!("只看错误 {}", self.errors);
                        if chip(ui, &label, self.errors_only).clicked() {
                            self.errors_only = !self.errors_only;
                        }
                        let search = search_field(ui, &mut self.query, "搜索日志", 180.0);
                        if search.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            self.query.clear();
                        }
                    });
                });

                ui.add_space(8.0);
                egui::Frame::new()
                    .fill(t::bg())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_height(body_h);
                        if self.shown.is_empty() {
                            ui.centered_and_justified(|ui| {
                                let empty = if self.lines.is_empty() {
                                    match self.tab {
                                        Tab::Game if running => "游戏还没有输出",
                                        Tab::Game => "这次启动没有留下输出",
                                        _ => "还没有记录",
                                    }
                                } else {
                                    "没有符合筛选的行"
                                };
                                ui.label(egui::RichText::new(empty).size(13.0).color(t::dim()));
                            });
                            return;
                        }
                        let font = egui::FontId::monospace(12.5);
                        let row_h = ui.fonts_mut(|f| f.row_height(&font)) + 2.0;
                        egui::ScrollArea::both()
                            .id_salt(("log_lines", self.tab as u8))
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .show_rows(ui, row_h, self.shown.len(), |ui, range| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for &index in &self.shown[range] {
                                    let line = &self.lines[index];
                                    let color = if launch::is_error_line(line) {
                                        t::danger()
                                    } else if is_warn_line(line) {
                                        t::warn()
                                    } else {
                                        t::sub()
                                    };
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(line)
                                                .font(font.clone())
                                                .color(color),
                                        )
                                        .wrap_mode(egui::TextWrapMode::Extend),
                                    );
                                }
                            });
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let info = match &path {
                        Some(path) => format!(
                            "{} · {}",
                            path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            human_size(self.len)
                        ),
                        None => format!("{} 条", self.lines.len()),
                    };
                    ui.label(egui::RichText::new(info).size(13.0).color(t::dim()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(path) = &path {
                            if action_link(ui, "在文件夹中显示").clicked() {
                                reveal_file(path);
                            }
                            if action_link(ui, "用编辑器打开").clicked() {
                                open_folder(path);
                            }
                        }
                        let copy_label = if self.errors_only || !self.query.trim().is_empty() {
                            "复制显示的行"
                        } else {
                            "复制全部"
                        };
                        if action_link(ui, copy_label)
                            .on_hover_text("贴给别人帮忙看时用")
                            .clicked()
                        {
                            let text = self
                                .shown
                                .iter()
                                .map(|&index| self.lines[index].as_str())
                                .collect::<Vec<_>>()
                                .join("\n");
                            ui.ctx().copy_text(text);
                        }
                    });
                });
            });
        if let Some(tab) = wanted_tab {
            self.tab = tab;
            self.errors_only = false;
        }
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        close
    }
}

impl App {
    /// 打开日志窗口。游戏异常退出时直接是这次的游戏输出，启动器自己出的错是启动器记录。
    pub(crate) fn open_log(&mut self) {
        let game = match (&self.play.last, &self.error) {
            (Some(_), Some(error)) => error.starts_with("游戏异常退出"),
            (Some(last), None) => !matches!(last.outcome, log::Outcome::Failed(_)),
            (None, _) => false,
        };
        self.show_settings = false;
        self.login.open = false;
        self.log_view = Some(log::LogView::new(if game {
            log::Tab::Game
        } else {
            log::Tab::Launcher
        }));
    }

    pub(crate) fn log_overlay(&mut self, ctx: &egui::Context) {
        let Some(view) = &mut self.log_view else {
            return;
        };
        if view.show(ctx, self.play.last.as_ref(), &self.log) {
            self.log_view = None;
        }
    }
}
