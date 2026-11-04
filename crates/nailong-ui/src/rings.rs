//! 状态圆环：启动台右侧的图形，显示这个版本现在的情况。
//!
//! 从内到外是中心圆点、文件环、Mod 环和快照刻度。平时不带文字，悬停或用键盘
//! 聚焦某一层时才亮起并显示说明。除了安装中和游戏运行中，它都是静止的。

use eframe::egui;
use egui::{Color32, Pos2, Sense, Shape, Stroke, pos2, vec2};
use std::f32::consts::TAU;

use crate::motion;
use crate::theme as t;

/// 设计尺寸，实际按区域高度缩放。
const DESIGN_SIZE: f32 = 260.0;
const FILE_R: f32 = 42.0;
const MOD_R: f32 = 72.0;
const TICK_R: f32 = 100.0;
/// Mod 超过这个数量后改成分段弧。
const MAX_DOTS: usize = 40;
const TICK_COUNT: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Core,
    Files,
    Mods,
    Saves,
}

const LAYERS: [Layer; 4] = [Layer::Core, Layer::Files, Layer::Mods, Layer::Saves];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Missing,
    Installing,
    Running,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ModDot {
    pub enabled: bool,
    pub update: bool,
}

pub struct Data {
    pub state: State,
    /// 文件完整度或安装进度，0..=1。
    pub fraction: f32,
    pub missing: usize,
    /// 安装时显示的文件和字节进度。
    pub progress_detail: String,
    pub mods: Vec<ModDot>,
    pub update_names: Vec<String>,
    pub snapshots: usize,
    pub latest_snapshot: Option<String>,
    pub running_minutes: u64,
}

pub struct Tip {
    pub title: String,
    pub detail: String,
    pub action: &'static str,
}

/// 某一层的说明文字。
pub fn tip(data: &Data, layer: Layer) -> Tip {
    let enabled = data.mods.iter().filter(|m| m.enabled).count();
    let disabled = data.mods.len() - enabled;
    let (title, detail, action): (String, String, &'static str) = match (layer, data.state) {
        (Layer::Core, State::Ready) => (
            "可以启动".into(),
            if data.update_names.is_empty() {
                "所有文件都已就绪。".into()
            } else {
                "有更新不影响启动。".into()
            },
            "点击开始游戏",
        ),
        (Layer::Core, State::Missing) => (
            "暂时不能启动".into(),
            format!("还缺 {} 个文件，补全后就能玩。", data.missing),
            "点击补全文件",
        ),
        (Layer::Core, State::Installing) => {
            ("正在安装".into(), "可以最小化窗口，安装会继续。".into(), "")
        }
        (Layer::Core, State::Running) => (
            "游戏运行中".into(),
            if data.running_minutes == 0 {
                "刚刚启动。".into()
            } else {
                format!("已运行 {} 分钟。", data.running_minutes)
            },
            "点击查看日志",
        ),
        (Layer::Files, State::Missing) => (
            format!("缺 {} 个文件", data.missing),
            "补全时只下载缺少的部分。".into(),
            "点击查看版本详情",
        ),
        (Layer::Files, State::Installing) => (
            format!("安装进度 {:.0}%", data.fraction * 100.0),
            data.progress_detail.clone(),
            "",
        ),
        (Layer::Files, State::Running) => (
            "文件完整".into(),
            "运行期间不会改动游戏文件。".into(),
            "点击查看版本详情",
        ),
        (Layer::Files, _) => (
            "文件完整".into(),
            "客户端、运行库和资源都没有缺失。".into(),
            "点击查看版本详情",
        ),
        (Layer::Mods, State::Installing) => {
            ("等待安装".into(), "装好游戏后再处理 Mod。".into(), "")
        }
        (Layer::Mods, _) if data.mods.is_empty() => (
            "还没有 Mod".into(),
            "可以去内容页发现和安装。".into(),
            "点击打开内容",
        ),
        (Layer::Mods, State::Running) => (
            format!("{} 个 Mod", data.mods.len()),
            "运行中可以查看，退出游戏后才能修改。".into(),
            "点击查看内容",
        ),
        (Layer::Mods, _) if !data.update_names.is_empty() => (
            format!("{} 个可更新", data.update_names.len()),
            format!("{}。更新前会留恢复点。", data.update_names.join("、")),
            "点击查看更新",
        ),
        (Layer::Mods, _) => (
            format!("{} 个 Mod", data.mods.len()),
            format!("{enabled} 个已启用，{disabled} 个已禁用。"),
            "点击打开内容",
        ),
        (Layer::Saves, _) if data.snapshots == 0 => (
            "还没有快照".into(),
            "可以在存档页给世界做备份。".into(),
            "点击打开存档",
        ),
        (Layer::Saves, _) => (
            format!("{} 个快照", data.snapshots),
            data.latest_snapshot
                .as_ref()
                .map(|age| format!("最新一份在 {age}，新的刻度在最上方。"))
                .unwrap_or_default(),
            "点击打开存档",
        ),
    };
    Tip {
        title,
        detail,
        action,
    }
}

/// 根据到圆心的距离判断鼠标在哪一层（设计坐标）。
pub fn layer_at(distance: f32) -> Option<Layer> {
    match distance {
        d if d < 24.0 => Some(Layer::Core),
        d if d < 58.0 => Some(Layer::Files),
        d if d < 88.0 => Some(Layer::Mods),
        d if d <= 122.0 => Some(Layer::Saves),
        _ => None,
    }
}

/// 第 i 个快照的刻度位置：新的在最上方，按顺时针排开。
pub fn snapshot_tick(index: usize, total: usize) -> usize {
    let step = (TICK_COUNT / total.max(12)).max(1);
    index * step
}

fn at(center: Pos2, angle: f32, radius: f32) -> Pos2 {
    pos2(
        center.x + radius * angle.sin(),
        center.y - radius * angle.cos(),
    )
}

fn arc(center: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let steps = (((to - from).abs() / TAU * 96.0).ceil() as usize).max(2);
    (0..=steps)
        .map(|index| {
            at(
                center,
                egui::lerp(from..=to, index as f32 / steps as f32),
                radius,
            )
        })
        .collect()
}

/// 画圆环。点击某一层时返回它。
pub fn show(ui: &mut egui::Ui, rect: egui::Rect, data: &Data) -> Option<Layer> {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique("nailong_status_rings");
    let response = ui.interact(rect, id, Sense::click());
    let center = rect.center();
    let scale = rect.height() / DESIGN_SIZE;

    // 方向键在各层之间切换，Enter 或空格触发。
    let focus_id = id.with("layer");
    let mut focused_layer = ctx.data(|d| d.get_temp::<usize>(focus_id)).unwrap_or(0);
    if response.has_focus() {
        ctx.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (next, previous) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowRight) || input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowUp),
            )
        });
        if next {
            focused_layer = (focused_layer + 1) % LAYERS.len();
        }
        if previous {
            focused_layer = (focused_layer + LAYERS.len() - 1) % LAYERS.len();
        }
        ctx.data_mut(|d| d.insert_temp(focus_id, focused_layer));
    }
    let pointer_layer = response
        .hover_pos()
        .and_then(|pos| layer_at((pos - center).length() / scale));
    let hot = pointer_layer.or(response.has_focus().then_some(LAYERS[focused_layer]));
    if pointer_layer.is_some() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // 每层的高亮程度。
    let amount = |layer: Layer| {
        if motion::reduced() {
            return if hot == Some(layer) { 1.0 } else { 0.0 };
        }
        ctx.animate_bool_with_time(id.with(("hot", layer as u8)), hot == Some(layer), 0.18)
    };
    let hot_amounts = LAYERS.map(amount);
    let visibility = |layer: Layer| {
        let others = LAYERS
            .iter()
            .zip(hot_amounts)
            .filter(|(other, _)| **other != layer)
            .map(|(_, value)| value)
            .fold(0.0_f32, f32::max);
        1.0 - others * 0.78
    };

    let warn_amount = if motion::reduced() {
        f32::from(data.state == State::Missing)
    } else {
        ctx.animate_bool_with_time(id.with("warn"), data.state == State::Missing, 0.4)
    };
    let state_color = motion::mix(t::accent(), t::warn(), warn_amount);
    let time = ctx.input(|input| input.time) as f32;
    let painter = ui.painter_at(rect.expand(8.0));

    // 运行中：从中心缓慢向外扩散。窗口失焦时不动。
    let focused = ctx.input(|input| input.focused);
    if data.state == State::Running && !motion::reduced() && focused {
        let phase = (time / 2.6).rem_euclid(1.0);
        painter.circle_stroke(
            center,
            (24.0 + phase * 96.0) * scale,
            Stroke::new(1.0, t::tint(state_color, (1.0 - phase) * 0.6)),
        );
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    // 快照刻度
    let saves_hot = hot_amounts[3];
    let saves_spring = motion::spring(
        &ctx,
        id.with("saves"),
        if hot == Some(Layer::Saves) { 1.0 } else { 0.0 },
        150.0,
        16.0,
    );
    let saves_fade = visibility(Layer::Saves);
    if data.snapshots == 0 {
        painter.circle_stroke(
            center,
            TICK_R * scale,
            Stroke::new(1.0, t::tint(t::hairline(), saves_fade)),
        );
    } else {
        let spin = saves_spring * 6.0_f32.to_radians();
        let grow = 1.0 + saves_spring * 0.05;
        let marks: Vec<usize> = (0..data.snapshots.min(TICK_COUNT))
            .map(|index| snapshot_tick(index, data.snapshots))
            .collect();
        for tick in 0..TICK_COUNT {
            let angle = tick as f32 / TICK_COUNT as f32 * TAU + spin;
            let major = marks.contains(&tick);
            let (length, color) = if tick == 0 {
                (14.0, t::accent())
            } else if major {
                (14.0, t::dot())
            } else {
                (5.0, t::hairline())
            };
            painter.line_segment(
                [
                    at(center, angle, TICK_R * grow * scale),
                    at(center, angle, (TICK_R + length) * grow * scale),
                ],
                Stroke::new(1.0 + saves_hot * 0.4, t::tint(color, saves_fade)),
            );
        }
    }

    // Mod 环
    let mods_fade = visibility(Layer::Mods);
    let mods_raw = hot_amounts[2];
    let waiting = data.state == State::Installing;
    painter.circle_stroke(
        center,
        MOD_R * scale,
        Stroke::new(1.0, t::tint(t::hairline(), mods_fade)),
    );
    let colour_of = |dot: &ModDot| -> (Color32, bool) {
        if waiting {
            (t::dim(), false)
        } else if dot.update {
            (t::accent(), true)
        } else if dot.enabled {
            (t::dot(), true)
        } else {
            (t::dim(), false)
        }
    };
    if data.mods.len() > MAX_DOTS {
        // 太多时按比例分段：可更新、启用、禁用。
        let total = data.mods.len() as f32;
        let updates = data.mods.iter().filter(|m| m.update).count() as f32;
        let enabled = data.mods.iter().filter(|m| m.enabled && !m.update).count() as f32;
        let mut from = 0.0;
        for (count, color) in [
            (updates, t::accent()),
            (enabled, t::dot()),
            (total - updates - enabled, t::dim()),
        ] {
            let to = from + count / total * TAU;
            if count > 0.0 {
                painter.add(Shape::line(
                    arc(center, MOD_R * scale, from + 0.02, to - 0.02),
                    Stroke::new((3.0 + mods_raw * 2.0) * scale, t::tint(color, mods_fade)),
                ));
            }
            from = to;
        }
    } else {
        let count = data.mods.len().max(1) as f32;
        for (index, dot) in data.mods.iter().enumerate() {
            let angle = index as f32 / count * TAU + 0.25;
            // 依次弹开：后面的点晚一点动。
            let delay = index as f32 / count * 0.35;
            let local = ((mods_raw - delay) / 0.65).clamp(0.0, 1.0);
            let back = local + (local * std::f32::consts::PI).sin() * 0.25;
            let pos = at(center, angle, (MOD_R + back * 8.0) * scale);
            let (color, filled) = colour_of(dot);
            let radius = if dot.update && !waiting { 6.0 } else { 4.5 } * scale;
            let color = t::tint(color, mods_fade);
            painter.circle(
                pos,
                radius,
                if filled {
                    color
                } else {
                    t::tint(t::bg(), mods_fade)
                },
                Stroke::new(1.0, color),
            );
        }
    }

    // 文件环
    let files_fade = visibility(Layer::Files);
    painter.circle_stroke(
        center,
        FILE_R * scale,
        Stroke::new(1.0, t::tint(t::hairline(), files_fade)),
    );
    let fraction = motion::spring(
        &ctx,
        id.with("fraction"),
        data.fraction.clamp(0.0, 1.0),
        40.0,
        12.0,
    );
    if fraction > 0.002 {
        painter.add(Shape::line(
            arc(center, FILE_R * scale, 0.0, fraction.min(1.0) * TAU),
            Stroke::new(
                (3.0 + hot_amounts[1] * 3.0) * scale,
                t::tint(state_color, files_fade),
            ),
        ));
    }
    if waiting && !motion::reduced() {
        let spin = (time / 2.4).rem_euclid(1.0) * TAU;
        for dash in 0..12 {
            let from = spin + dash as f32 / 12.0 * TAU;
            painter.add(Shape::line(
                arc(center, 54.0 * scale, from, from + 0.2),
                Stroke::new(1.0, t::tint(t::dim(), files_fade)),
            ));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    // 中心圆点
    let core = motion::spring(
        &ctx,
        id.with("core"),
        if hot == Some(Layer::Core) { 17.0 } else { 12.0 },
        420.0,
        20.0,
    );
    painter.circle_filled(
        center,
        core.max(1.0) * scale,
        t::tint(state_color, visibility(Layer::Core)),
    );

    if let Some(layer) = hot {
        paint_tip(&ctx, rect, &tip(data, layer));
    }

    if response.clicked() {
        pointer_layer.or(response.has_focus().then_some(LAYERS[focused_layer]))
    } else {
        None
    }
}

/// 说明卡片浮在圆环左边，不遮挡圆环的主体。
fn paint_tip(ctx: &egui::Context, rect: egui::Rect, tip: &Tip) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("nailong_ring_tip"),
    ));
    let width = 180.0;
    let pad = 12.0;
    let inner = width - pad * 2.0;
    let title = painter.layout(
        tip.title.clone(),
        egui::FontId::new(14.0, crate::bold_family()),
        t::ink(),
        inner,
    );
    let detail = painter.layout(
        tip.detail.clone(),
        egui::FontId::proportional(13.0),
        t::sub(),
        inner,
    );
    let action = painter.layout(
        tip.action.to_string(),
        egui::FontId::proportional(13.0),
        t::accent(),
        inner,
    );
    let mut height = pad * 2.0 + title.size().y + 4.0 + detail.size().y;
    if !tip.action.is_empty() {
        height += 6.0 + action.size().y;
    }
    let min = pos2(rect.left() - width + 28.0, rect.center().y - height * 0.5);
    let card = egui::Rect::from_min_size(min, vec2(width, height));
    painter.rect(
        card,
        motion::R,
        t::surface(),
        Stroke::new(1.0, t::hairline()),
        egui::StrokeKind::Inside,
    );
    let mut y = card.top() + pad;
    let left = card.left() + pad;
    let title_h = title.size().y;
    painter.galley(pos2(left, y), title, t::ink());
    y += title_h + 4.0;
    let detail_h = detail.size().y;
    painter.galley(pos2(left, y), detail, t::sub());
    y += detail_h + 6.0;
    if !tip.action.is_empty() {
        painter.galley(pos2(left, y), action, t::accent());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(state: State) -> Data {
        Data {
            state,
            fraction: 1.0,
            missing: 12,
            progress_detail: "812 / 3911 个文件".into(),
            mods: vec![
                ModDot {
                    enabled: true,
                    update: true,
                },
                ModDot {
                    enabled: true,
                    update: false,
                },
                ModDot {
                    enabled: false,
                    update: false,
                },
            ],
            update_names: vec!["Create".into()],
            snapshots: 2,
            latest_snapshot: Some("2 小时前".into()),
            running_minutes: 12,
        }
    }

    #[test]
    fn distance_maps_to_the_expected_layer() {
        assert_eq!(layer_at(0.0), Some(Layer::Core));
        assert_eq!(layer_at(42.0), Some(Layer::Files));
        assert_eq!(layer_at(72.0), Some(Layer::Mods));
        assert_eq!(layer_at(110.0), Some(Layer::Saves));
        assert_eq!(layer_at(130.0), None);
    }

    #[test]
    fn tips_follow_the_state() {
        assert_eq!(tip(&data(State::Ready), Layer::Core).title, "可以启动");
        assert_eq!(
            tip(&data(State::Missing), Layer::Files).title,
            "缺 12 个文件"
        );
        assert_eq!(tip(&data(State::Ready), Layer::Mods).title, "1 个可更新");
        assert_eq!(tip(&data(State::Installing), Layer::Mods).action, "");
        assert!(
            tip(&data(State::Running), Layer::Core)
                .detail
                .contains("12 分钟")
        );
    }

    #[test]
    fn newest_snapshot_sits_at_the_top_and_ticks_stay_in_range() {
        assert_eq!(snapshot_tick(0, 5), 0);
        for total in [1, 5, 12, 60, 300] {
            for index in 0..total.min(TICK_COUNT) {
                assert!(snapshot_tick(index, total) < TICK_COUNT, "{index}/{total}");
            }
        }
    }
}
