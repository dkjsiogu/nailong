//! 从真实游戏里学这个版本需要多少内存。
//!
//! 每次启动都让 JVM 把回收记录写进游戏目录的 `logs/nailong-gc.log`（一局只有几百 KB）。
//! 游戏退出后读出清理过老年代的回收之后剩下多少——这就是这套 Mod、这个世界、
//! 这个视距下真正常驻的数据。下次启动「自动」内存按它算，比按 Mod 数量估准。
//!
//! 一局里没发生过老年代回收（玩得短、堆给得大）时不知道存活多少，不瞎猜；
//! 出现整堆回收或晋升失败说明堆偏小，下次至少加 1/4。

use crate::gclog;
use crate::java::JavaRuntime;
use crate::paths::Root;
use crate::tuning::{Collector, Learned};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 相对游戏目录。游戏以游戏目录为工作目录启动，相对路径里没有盘符的冒号，
/// 不会和 `-Xlog` 自己的分隔符冲突。
pub const GC_LOG: &str = "logs/nailong-gc.log";
const STORE: &str = "learned.json";
/// 每个版本只记最近几局。
const KEEP: usize = 5;
/// 启动后这么久之内的回收不算存活数据：那时在加载 Mod 和资源，还没进世界，
/// 元数据触发的混合回收会量出一个偏小的数。
const SETTLE_SECS: f64 = 180.0;

/// 一局游戏学到的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    /// Unix 秒。
    pub when: u64,
    pub heap_mb: u32,
    pub collector: Collector,
    /// 最后一条回收记录离启动多少分钟，大致等于这局玩了多久。
    pub minutes: f64,
    pub live_mb: Option<u32>,
    pub full: usize,
    pub evac_failures: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    /// Mod 列表的指纹；换了 Mod，存活数据就不再作数。
    fingerprint: String,
    sessions: Vec<Session>,
}

/// `-Xlog` 从 Java 9 开始才有，而且是 HotSpot 的写法；Java 8 和 OpenJ9 只按公式算。
pub fn jvm_arg(java: &JavaRuntime) -> Option<String> {
    let openj9 = java.vendor.as_deref().is_some_and(|vendor| {
        let vendor = vendor.to_ascii_lowercase();
        vendor.contains("openj9") || vendor.contains("ibm") || vendor.contains("semeru")
    });
    (java.major >= 9 && !openj9).then(|| format!("-Xlog:gc:file={GC_LOG}:uptime:filecount=0"))
}

/// 启动前：日志目录要先在，否则 JVM 打不开文件会直接拒绝启动。
pub fn prepare(game_dir: &Path) {
    let path = game_dir.join(GC_LOG);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(path);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 启动时用绝对路径，界面预览时可能是相对路径，统一成绝对路径才能对上。
fn key(profile: &str, game_dir: &Path) -> String {
    let dir = std::path::absolute(game_dir).unwrap_or_else(|_| game_dir.to_path_buf());
    format!("{profile}|{}", dir.display())
}

fn store_path(root: &Root) -> PathBuf {
    root.dir().join(STORE)
}

fn load_all(root: &Root) -> BTreeMap<String, Record> {
    std::fs::read(store_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 只看 `mods` 里启用的 jar：名字和大小。
pub fn fingerprint(game_dir: &Path) -> String {
    let mut mods: Vec<(String, u64)> = std::fs::read_dir(game_dir.join("mods"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.to_ascii_lowercase()
                .ends_with(".jar")
                .then(|| (name, entry.metadata().map(|m| m.len()).unwrap_or(0)))
        })
        .collect();
    mods.sort();
    let mut hash = Sha1::new();
    for (name, size) in mods {
        hash.update(format!("{name}:{size}\n"));
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 从一局的 GC 日志里读出这一局。
pub fn session_from_log(text: &str, heap_mb: u32, collector: Collector) -> Option<Session> {
    let events = gclog::parse(text);
    let last = events.last()?;
    let summary = gclog::summarize(&events, 0.0, f64::MAX);
    Some(Session {
        when: now(),
        heap_mb,
        collector,
        minutes: last.at / 60.0,
        live_mb: gclog::live_set(
            &events
                .iter()
                .filter(|e| e.at >= SETTLE_SECS)
                .cloned()
                .collect::<Vec<_>>(),
        ),
        full: summary.full,
        evac_failures: summary.evac_failures,
    })
}

/// 游戏退出后调用：读日志、记下这一局。读不到日志（Java 8、启动失败）时什么都不做。
pub fn record(
    root: &Root,
    profile: &str,
    game_dir: &Path,
    heap_mb: u32,
    collector: Collector,
) -> Option<Session> {
    let text = std::fs::read_to_string(game_dir.join(GC_LOG)).ok()?;
    let session = session_from_log(&text, heap_mb, collector)?;
    let mut all = load_all(root);
    let print = fingerprint(game_dir);
    let record = all.entry(key(profile, game_dir)).or_default();
    if record.fingerprint != print {
        record.fingerprint = print;
        record.sessions.clear();
    }
    record.sessions.push(session.clone());
    let excess = record.sessions.len().saturating_sub(KEEP);
    record.sessions.drain(..excess);
    let path = store_path(root);
    if let Ok(bytes) = serde_json::to_vec_pretty(&all) {
        let _ = std::fs::write(path, bytes);
    }
    Some(session)
}

/// 这个版本学到的；换了 Mod 或从没玩过时为 None。
pub fn load(root: &Root, profile: &str, game_dir: &Path) -> Option<Learned> {
    let record = load_all(root).remove(&key(profile, game_dir))?;
    if record.fingerprint != fingerprint(game_dir) {
        return None;
    }
    summarize(&record.sessions)
}

fn summarize(sessions: &[Session]) -> Option<Learned> {
    let latest = sessions.iter().map(|s| s.when).max()?;
    let measured: Vec<u32> = sessions.iter().filter_map(|s| s.live_mb).collect();
    let floor = sessions
        .iter()
        .filter(|s| s.full > 0 || s.evac_failures > 0)
        .map(|s| (s.heap_mb + s.heap_mb / 4).div_ceil(512) * 512)
        .max();
    if measured.is_empty() && floor.is_none() {
        return None;
    }
    Some(Learned {
        live_mb: measured.iter().copied().max(),
        floor_mb: floor,
        sessions: measured.len(),
        age: crate::bench::age(latest),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(live: Option<u32>, heap: u32, full: usize) -> Session {
        Session {
            when: now(),
            heap_mb: heap,
            collector: Collector::G1,
            minutes: 20.0,
            live_mb: live,
            full,
            evac_failures: 0,
        }
    }

    #[test]
    fn reads_a_session_from_a_game_log() {
        let log = "\
[0.010s] Using G1
[2.647s] GC(3) Pause Young (Mixed) (Metadata GC Threshold) 110M->69M(4096M) 10.690ms
[5.000s] GC(0) Pause Young (Normal) (G1 Evacuation Pause) 900M->300M(4096M) 12.000ms
[600.000s] GC(40) Pause Young (Mixed) (G1 Evacuation Pause) 3000M->1800M(4096M) 20.000ms
[1200.000s] GC(80) Pause Young (Mixed) (G1 Evacuation Pause) 3100M->1700M(4096M) 18.000ms";
        let s = session_from_log(log, 4096, Collector::G1).unwrap();
        assert_eq!(s.live_mb, Some(1800));
        assert!((s.minutes - 20.0).abs() < 1e-9);
        assert_eq!(s.full, 0);
        // 只停在加载阶段，或者只做过新生代回收：不知道存活多少。
        let short = session_from_log(
            "[5.000s] GC(0) Pause Young (Normal) (G1 Evacuation Pause) 900M->300M(4096M) 12.000ms",
            4096,
            Collector::G1,
        )
        .unwrap();
        assert_eq!(short.live_mb, None);
        // 实测：NeoForge 加载时元数据触发的混合回收，只停在主菜单的一局不算。
        let loading = session_from_log(
            "[2.647s] GC(3) Pause Young (Mixed) (Metadata GC Threshold) 110M->69M(3584M) 10.690ms\n\
             [6.193s] GC(9) Pause Young (Prepare Mixed) (G1 Evacuation Pause) 782M->188M(3584M) 23.672ms",
            3584,
            Collector::G1,
        )
        .unwrap();
        assert_eq!(loading.live_mb, None);
        assert!(session_from_log("[0.010s] Using G1", 4096, Collector::G1).is_none());
    }

    #[test]
    fn keeps_the_largest_live_set_and_a_floor_after_pressure() {
        assert!(summarize(&[]).is_none());
        assert!(summarize(&[session(None, 4096, 0)]).is_none());
        let learned = summarize(&[
            session(Some(1800), 4096, 0),
            session(Some(2300), 4096, 0),
            session(None, 3072, 1),
        ])
        .unwrap();
        assert_eq!(learned.live_mb, Some(2300));
        assert_eq!(learned.sessions, 2);
        // 3 GB 下出过整堆回收：至少 3.75 GB，向上取到 4 GB。
        assert_eq!(learned.floor_mb, Some(4096));
    }

    #[test]
    fn records_per_version_and_forgets_after_mod_changes() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("nailong-learned-{}-{n}", std::process::id()));
        let root = Root::new(base.join("data"));
        std::fs::create_dir_all(root.dir()).unwrap();
        let game = base.join("game");
        std::fs::create_dir_all(game.join("mods")).unwrap();
        std::fs::write(game.join("mods/a.jar"), b"a").unwrap();
        prepare(&game);
        assert!(game.join("logs").is_dir());
        std::fs::write(
            game.join(GC_LOG),
            "[900.000s] GC(9) Pause Young (Mixed) (G1 Evacuation Pause) 2500M->1500M(4096M) 9.000ms",
        )
        .unwrap();
        record(&root, "pack", &game, 4096, Collector::G1).unwrap();
        assert_eq!(load(&root, "pack", &game).unwrap().live_mb, Some(1500));
        assert!(load(&root, "other", &game).is_none());
        std::fs::write(game.join("mods/b.jar"), b"b").unwrap();
        assert!(load(&root, "pack", &game).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
