//! 读 JVM 统一日志（`-Xlog:gc`）里的回收记录。通用测试和真实游戏都用它：
//! 前者算停顿，后者从清理过老年代的回收里读出存活数据。

use serde::{Deserialize, Serialize};

/// 一帧（60 FPS）的时长。超过它的停顿会被看成一次卡顿。
pub const FRAME_MS: f64 = 1000.0 / 60.0;

/// GC 日志里的一条记录。
#[derive(Debug, Clone, PartialEq)]
pub struct GcEvent {
    /// JVM 启动后的秒数。
    pub at: f64,
    /// 停顿时长；ZGC 的回收周期行没有停顿，为 None。
    pub pause_ms: Option<f64>,
    /// 回收后的堆占用。
    pub after_mb: Option<u32>,
    /// 这次回收是否清理了老年代：G1 的混合/整堆回收、ZGC 的大回收。
    pub old: bool,
    pub full: bool,
    pub evac_failure: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GcSummary {
    /// 计时段里的停顿次数。
    pub pauses: usize,
    pub total_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    /// 超过一帧的停顿次数。
    pub over_frame: usize,
    /// 整轮里的整堆回收次数。
    pub full: usize,
    /// 整轮里 G1 晋升失败（To-space exhausted / Evacuation Failure）的次数。
    pub evac_failures: usize,
}

pub fn parse(text: &str) -> Vec<GcEvent> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<GcEvent> {
    let rest = line.strip_prefix('[')?;
    let (uptime, rest) = rest.split_once("s]")?;
    let at: f64 = uptime.parse().ok()?;
    let body = rest.trim();
    let gc_pos = body.find("GC(")?;
    let body = &body[gc_pos..];
    let pause = body.contains("Pause ");
    let collection = body.contains("Collection (") || body.contains("Garbage Collection");
    // JDK 11 把晋升失败单独记一行：`GC(5) To-space exhausted`。
    let exhausted = body.contains("To-space exhausted");
    if !pause && !collection && !exhausted {
        return None;
    }
    let pause_ms = if pause {
        body.rsplit(' ')
            .next()
            .and_then(|t| t.strip_suffix("ms"))
            .and_then(|t| t.parse().ok())
    } else {
        None
    };
    // G1：`409M->72M(1024M)`；ZGC：`512M(6%)->128M(2%)`。
    let after_mb = body.split_once("->").and_then(|(_, tail)| {
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    });
    if pause_ms.is_none() && after_mb.is_none() && !exhausted {
        return None;
    }
    let full = body.contains("Pause Full");
    Some(GcEvent {
        at,
        pause_ms,
        after_mb,
        old: full || body.contains("(Mixed)") || body.contains("Major Collection"),
        full,
        evac_failure: body.contains("Evacuation Failure") || exhausted,
    })
}

/// `from..=to` 秒之间的停顿；整堆回收和晋升失败按整份日志算。
pub fn summarize(events: &[GcEvent], from: f64, to: f64) -> GcSummary {
    let mut pauses: Vec<f64> = events
        .iter()
        .filter(|e| e.at >= from && e.at <= to)
        .filter_map(|e| e.pause_ms)
        .collect();
    pauses.sort_by(|a, b| a.total_cmp(b));
    let pick = |q: f64| {
        pauses
            .get(((pauses.len() as f64 * q) as usize).min(pauses.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0.0)
    };
    GcSummary {
        pauses: pauses.len(),
        total_ms: pauses.iter().sum(),
        p95_ms: pick(0.95),
        max_ms: pauses.last().copied().unwrap_or(0.0),
        over_frame: pauses.iter().filter(|&&ms| ms > FRAME_MS).count(),
        full: events.iter().filter(|e| e.full).count(),
        evac_failures: events.iter().filter(|e| e.evac_failure).count(),
    }
}

/// 存活数据：清理过老年代的回收之后剩下的最大值。没有这类回收时不知道，
/// 只做过新生代回收的占用里混着还没清理的老年代垃圾，拿来算会偏大。
pub fn live_set(events: &[GcEvent]) -> Option<u32> {
    events
        .iter()
        .filter(|e| e.old)
        .filter_map(|e| e.after_mb)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_g1_and_zgc_logs() {
        let log = "\
[1.160s] GC(0) Pause Young (Concurrent Start) (Metadata GC Threshold) 400M->45M(3584M) 8.202ms
[30.500s] GC(7) Pause Young (Normal) (G1 Evacuation Pause) 900M->300M(3584M) 21.500ms
[31.000s] GC(8) Pause Remark 320M->310M(3584M) 2.100ms
[32.000s] GC(9) Pause Young (Mixed) (G1 Evacuation Pause) 700M->260M(3584M) 9.000ms
[40.000s] GC(10) Pause Young (Normal) (G1 Evacuation Pause) (Evacuation Failure: Allocation) 3500M->3400M(3584M) 80.000ms
[41.000s] GC(11) Pause Full (G1 Compaction Pause) 3500M->1200M(3584M) 400.000ms
[50.100s] GC(12) y: Pause Mark Start 0.010ms
[50.300s] GC(12) Major Collection (Allocation Rate) 1500M(20%)->900M(12%) 0.350s
[50.400s] GC(13) Concurrent Mark 12.000ms";
        let events = parse(log);
        assert_eq!(events.len(), 8);
        let route = summarize(&events, 30.0, 45.0);
        assert_eq!(route.pauses, 5);
        assert_eq!(route.over_frame, 3);
        assert_eq!(route.full, 1);
        assert_eq!(route.evac_failures, 1);
        assert!((route.max_ms - 400.0).abs() < 1e-9);
        // 清理过老年代的回收后最大：整堆回收后的 1200M。
        assert_eq!(live_set(&events), Some(1200));
        // 只有新生代回收时不猜。
        assert_eq!(live_set(&events[..2]), None);
    }

    #[test]
    fn reads_jdk11_evacuation_failures_and_default_decorations() {
        let log = "\
[12.000s][info][gc] GC(3) To-space exhausted
[12.010s][info][gc] GC(3) Pause Young (Normal) (G1 Evacuation Pause) 2000M->1990M(2048M) 30.000ms";
        let events = parse(log);
        assert_eq!(events.len(), 2);
        assert_eq!(summarize(&events, 0.0, 99.0).evac_failures, 1);
        assert_eq!(events[1].after_mb, Some(1990));
    }
}
