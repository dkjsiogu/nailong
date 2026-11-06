//! 通用组件：按钮、胶囊、输入框、搜索框、路径行、列表行、标签，以及打开文件夹这类小工具。页面只拼装它们。

use crate::prelude::*;

/// 内容页的「已安装 / 发现」主模式：橙色底、深色字，一眼能看出当前在哪。
/// Four-block pixel spinner. Used anywhere the old egui arc spinner was;
/// it keeps the whole shell on the same grid and only requests frames while
/// it is actually visible.
pub(crate) fn busy_spinner(ui: &mut egui::Ui) {
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

pub(crate) fn content_tab(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    active: bool,
) -> egui::Response {
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
pub(crate) fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
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

/// 下载量用中文单位，避免卡片里塞一长串数字。
/// 卡片上只留一小段介绍，完整描述在详情页。
pub(crate) fn short_text(text: &str, max_chars: usize) -> String {
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

pub(crate) fn compact(count: u64) -> String {
    if count >= 100_000_000 {
        format!("{:.1}亿", count as f64 / 100_000_000.0)
    } else if count >= 10_000 {
        format!("{:.1}万", count as f64 / 10_000.0)
    } else {
        count.to_string()
    }
}

pub(crate) fn quick_action(ui: &mut egui::Ui, text: &str) -> egui::Response {
    motion::secondary_sized(ui, text, true, 94.0, 30.0, 11.0)
}

pub(crate) fn action_link(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE),
    )
}

pub(crate) fn status_dot(ui: &mut egui::Ui, ok: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 3.5, if ok { t::ok() } else { t::warn() });
}

pub(crate) fn inline_dot(ui: &mut egui::Ui) {
    ui.add_space(7.0);
    ui.label(egui::RichText::new("·").size(13.0).color(t::sub()));
    ui.add_space(7.0);
}

pub(crate) fn stat_cell(ui: &mut egui::Ui, value: usize, label: &str) {
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

pub(crate) fn info_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.0).color(t::sub()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(value).size(13.0));
        });
    });
}

/// 列表行的背景：当前项和悬停项都填 `wash`，不加色条。
pub(crate) fn list_row_fill(ui: &egui::Ui, rect: egui::Rect, active: bool, hovered: bool) {
    if active {
        ui.painter().rect_filled(rect, motion::R, t::wash());
    } else if hovered {
        ui.painter()
            .rect_filled(rect, motion::R, t::tint(t::wash(), 0.7));
    }
}

pub(crate) fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(1.0, t::hairline()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::same(18))
}

/// 加载骨架：不伪装成真实内容，只给一个稳定的占位高度。
pub(crate) fn skeleton_line(ui: &mut egui::Ui, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, height / 2.0, t::wash());
}

pub(crate) fn ghost_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(58.0, 28.0)),
    )
}

pub(crate) fn open_folder(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// 在文件管理器里打开所在目录并选中这个文件；做不到时退回打开目录。
pub(crate) fn reveal_file(path: &std::path::Path) {
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
pub(crate) fn percent_path(path: &std::path::Path) -> String {
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

pub(crate) fn human_size(bytes: u64) -> String {
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

pub(crate) fn human_age(modified: u64) -> String {
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

pub(crate) fn escape_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Escape))
}

pub(crate) fn progress_detail(p: &Progress) -> String {
    if p.bytes_total > 0 {
        format!("{} / {}", bytes(p.bytes_done), bytes(p.bytes_total))
    } else if p.files_total > 0 {
        format!("{} / {} 个文件", p.files_done, p.files_total)
    } else {
        String::new()
    }
}

pub(crate) fn bytes(n: u64) -> String {
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

pub(crate) fn primary_wide(ui: &mut egui::Ui, text: &str, enabled: bool, width: f32) -> bool {
    motion::primary_sized(ui, text, enabled, width, 40.0, 13.5)
}

pub(crate) fn accent_outline(ui: &mut egui::Ui, text: &str) -> egui::Response {
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

pub(crate) fn text_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
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
pub(crate) fn elide_middle(
    ui: &egui::Ui,
    text: &str,
    font: &egui::FontId,
    max_width: f32,
) -> String {
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
pub(crate) fn path_row(ui: &mut egui::Ui, label: &str, path: &std::path::Path) -> bool {
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
pub(crate) fn search_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
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

pub(crate) fn loader_label(name: &str) -> String {
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
pub(crate) fn tag_pill(ui: &mut egui::Ui, text: &str, color: egui::Color32, fill: egui::Color32) {
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

pub(crate) fn tag(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    tag_fill(ui, text, color, t::wash());
}

pub(crate) fn tag_fill(ui: &mut egui::Ui, text: &str, color: egui::Color32, fill: egui::Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(13.0).color(color));
        });
}

pub(crate) fn small_accent(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::accent()))
            .fill(t::tint(t::accent(), 0.14))
            .stroke(egui::Stroke::new(1.0, t::tint(t::accent(), 0.71)))
            .corner_radius(motion::R)
            .min_size(egui::vec2(44.0, 24.0)),
    )
}

pub(crate) fn tiny_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
            .fill(t::bg())
            .stroke(egui::Stroke::new(1.0, t::hairline()))
            .corner_radius(motion::R)
            .min_size(egui::vec2(44.0, 24.0)),
    )
}

pub(crate) fn danger_tiny(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(13.0).color(t::danger()))
            .fill(t::tint(t::danger(), 0.14))
            .corner_radius(motion::R)
            .min_size(egui::vec2(64.0, 24.0)),
    )
}

pub(crate) fn short_date(text: &str) -> String {
    text.get(..10).unwrap_or(text).to_string()
}
