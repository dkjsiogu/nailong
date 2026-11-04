//! 实测调参：用一个测试世界自动跑几轮游戏，量出这个版本真正需要多少内存、
//! 哪个回收器停顿更少，结果存进数据目录，之后的「自动」就按实测值启动。
//!
//! 测试世界由内置模板生成，数据包让玩家以旁观模式每 tick 前进一格，
//! 持续生成新区块；标准输出里的标记告诉我们进世界、开始和结束的时刻。
//! 规范见 `docs/design/20-tuning-benchmark.md`。

use crate::cancel;
use crate::error::{Error, Result, io};
use crate::launch::{self, LaunchOptions};
use crate::paths::Root;
use crate::progress::{Progress, Sink};
use crate::tuning::{Collector, GcChoice, Machine, Measured, gb};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WORLD: &str = "nailong-bench";
const GC_LOG: &str = "nailong-gc.log";
const GAME_LOG: &str = "nailong-bench.log";
/// 进世界后先等 200 tick（10 秒）让加载收尾，再开始计时。
const WARMUP_TICKS: u32 = 200;
/// 计时段：每 tick 前进一格，1200 tick 约飞过 75 个区块。
pub const ROUTE_TICKS: u32 = 1200;
const JOIN_TIMEOUT: Duration = Duration::from_secs(8 * 60);
/// G1 至少给新生代 20%、预留 20%，存活数据最多占堆的 60%。
const G1_LIVE_SHARE: f32 = 0.6;
/// ZGC 并发回收时还要接住新分配，存活数据最多占一半。
const ZGC_LIVE_SHARE: f32 = 0.5;
/// 一帧（60 FPS）的时长。超过它的停顿会被看成一次卡顿。
const FRAME_MS: f64 = 1000.0 / 60.0;

/// 1.20.1 创建的世界：1.20–1.21 打开时会静默升级。
const LEGACY_LEVEL: &[u8] = include_bytes!("../assets/bench/legacy/level.dat");
const LEGACY_DATA_VERSION: i32 = 3465;
/// 26.1 创建的世界。26.x 打开更旧的世界会弹出备份提示，所以按目标版本改写数据版本号。
const V26_DATA_VERSION: i32 = 4786;
const V26_FILES: &[(&str, &[u8])] = &[
    ("level.dat", include_bytes!("../assets/bench/v26/level.dat")),
    (
        "data/minecraft/custom_boss_events.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/custom_boss_events.dat"),
    ),
    (
        "data/minecraft/game_rules.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/game_rules.dat"),
    ),
    (
        "data/minecraft/random_sequences.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/random_sequences.dat"),
    ),
    (
        "data/minecraft/scheduled_events.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/scheduled_events.dat"),
    ),
    (
        "data/minecraft/scoreboard.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/scoreboard.dat"),
    ),
    (
        "data/minecraft/stopwatches.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/stopwatches.dat"),
    ),
    (
        "data/minecraft/weather.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/weather.dat"),
    ),
    (
        "data/minecraft/world_clocks.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/world_clocks.dat"),
    ),
    (
        "data/minecraft/world_gen_settings.dat",
        include_bytes!("../assets/bench/v26/data/minecraft/world_gen_settings.dat"),
    ),
];

/// 一轮测试的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    /// 例如「测量」「G1 验证」「ZGC 对比」。
    pub label: String,
    pub collector: Collector,
    pub heap_mb: u32,
    /// 从启动到进入世界的秒数。
    pub load_secs: f64,
    /// 计时段每秒完成的 tick，满速是 20。
    pub tps: f64,
    /// 计时段平均占用的 CPU 核数；读不到时为 None。
    pub cpu_cores: Option<f64>,
    pub gc: GcSummary,
    /// 整轮回收后存活数据的最大值。
    pub live_mb: Option<u32>,
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

/// 存进 `bench.json` 的一次实测。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub profile: String,
    pub game_dir: PathBuf,
    /// Mod 列表、Java 大版本和电脑内存的指纹；变了就说明结果过期。
    pub fingerprint: String,
    /// Unix 秒。
    pub when: u64,
    pub java_major: u32,
    pub live_mb: u32,
    pub g1_heap_mb: u32,
    pub zgc_heap_mb: Option<u32>,
    pub collector: Collector,
    pub runs: Vec<RunReport>,
    /// 结论的理由，一行一条。
    pub notes: Vec<String>,
}

impl Record {
    pub fn measured(&self) -> Measured {
        Measured {
            live_mb: self.live_mb,
            g1_heap_mb: self.g1_heap_mb,
            zgc_heap_mb: self.zgc_heap_mb,
            collector: self.collector,
            age: age(self.when),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 例如「今天」「3 天前」。不需要时区，比日期更好认。
pub fn age(when: u64) -> String {
    match now().saturating_sub(when) / 86_400 {
        0 => "今天".into(),
        1 => "昨天".into(),
        days => format!("{days} 天前"),
    }
}

fn key(profile: &str, game_dir: &Path) -> String {
    format!("{profile}|{}", game_dir.display())
}

fn store_path(root: &Root) -> PathBuf {
    root.dir().join("bench.json")
}

fn load_all(root: &Root) -> BTreeMap<String, Record> {
    std::fs::read(store_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(root: &Root, record: &Record) -> Result<()> {
    let mut all = load_all(root);
    all.insert(key(&record.profile, &record.game_dir), record.clone());
    let path = store_path(root);
    let bytes = serde_json::to_vec_pretty(&all).map_err(|e| Error::Launch(e.to_string()))?;
    std::fs::write(&path, bytes).map_err(|e| io(&path, e))
}

/// 这个版本的实测结果，以及它是否仍然有效。
pub fn load(
    root: &Root,
    profile: &str,
    game_dir: &Path,
    java_major: u32,
) -> Option<(Record, bool)> {
    let record = load_all(root).remove(&key(profile, game_dir))?;
    let fresh = record.fingerprint == fingerprint(profile, game_dir, java_major);
    Some((record, fresh))
}

/// 有效的实测结果，给调参用。
pub fn measured(root: &Root, profile: &str, game_dir: &Path, java_major: u32) -> Option<Measured> {
    load(root, profile, game_dir, java_major)
        .filter(|(_, fresh)| *fresh)
        .map(|(record, _)| record.measured())
}

/// 换了 Mod、换了 Java 大版本或换了内存条，实测结果就不再作数。
pub fn fingerprint(profile: &str, game_dir: &Path, java_major: u32) -> String {
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
    let total_gb = Machine::detect().total_mb.unwrap_or(0).div_ceil(1024);
    let mut hash = Sha1::new();
    hash.update(format!("{profile}\n{java_major}\n{total_gb}\n"));
    for (name, size) in mods {
        hash.update(format!("{name}:{size}\n"));
    }
    hex(&hash.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 客户端 jar 里 `version.json` 给出的世界数据版本和数据包格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GameInfo {
    world_version: i32,
    pack_format: u32,
}

fn game_info(client_jar: &Path) -> Result<GameInfo> {
    let fail = |why: &str| Error::Launch(format!("读不到游戏版本信息（{why}）"));
    let file = std::fs::File::open(client_jar).map_err(|e| io(client_jar, e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| fail("客户端不是有效的 jar"))?;
    let mut text = String::new();
    zip.by_name("version.json")
        .map_err(|_| fail("没有 version.json"))?
        .read_to_string(&mut text)
        .map_err(|_| fail("version.json 读取失败"))?;
    parse_game_info(&text).ok_or_else(|| fail("version.json 格式不认识"))
}

fn parse_game_info(text: &str) -> Option<GameInfo> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let world_version = value.get("world_version")?.as_i64()? as i32;
    let pack = value.get("pack_version")?;
    // 1.20–1.21.8 是 {"data": n}，1.21.9 起是 {"data_major": n, ...}，更早是单个数字。
    let pack_format = pack
        .get("data_major")
        .or_else(|| pack.get("data"))
        .and_then(|v| v.as_u64())
        .or_else(|| pack.as_u64())? as u32;
    Some(GameInfo {
        world_version,
        pack_format,
    })
}

/// 把 NBT 里的 `DataVersion` 和 `Version.Id` 改成目标版本，让游戏把它当成当前版本的世界。
fn patch_data_version(gzipped: &[u8], target: i32) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    flate2::read::GzDecoder::new(gzipped)
        .read_to_end(&mut data)
        .map_err(|e| Error::Launch(format!("测试世界模板损坏: {e}")))?;
    for name in [&b"DataVersion"[..], &b"Id"[..]] {
        let mut pattern = vec![3u8];
        pattern.extend((name.len() as u16).to_be_bytes());
        pattern.extend(name);
        let mut at = 0;
        while let Some(found) = find(&data[at..], &pattern) {
            let value = at + found + pattern.len();
            let old = i32::from_be_bytes(data[value..value + 4].try_into().unwrap());
            // 只改像数据版本号的 Id，别的 Id 字段保持原样。
            if name == b"DataVersion" || (3000..10000).contains(&old) {
                data[value..value + 4].copy_from_slice(&target.to_be_bytes());
            }
            at = value + 4;
        }
    }
    let mut out = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    out.write_all(&data)
        .and_then(|_| out.finish())
        .map_err(|e| Error::Launch(e.to_string()))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    std::fs::write(path, bytes).map_err(|e| io(path, e))
}

/// 在测试实例里写好测试世界和数据包。
fn prepare_world(game_dir: &Path, info: GameInfo) -> Result<()> {
    let world = game_dir.join("saves").join(WORLD);
    if world.exists() {
        std::fs::remove_dir_all(&world).map_err(|e| io(&world, e))?;
    }
    if info.world_version >= V26_DATA_VERSION {
        for (name, bytes) in V26_FILES {
            let bytes = if info.world_version == V26_DATA_VERSION {
                bytes.to_vec()
            } else {
                patch_data_version(bytes, info.world_version)?
            };
            write(&world.join(name), &bytes)?;
        }
    } else if info.world_version >= LEGACY_DATA_VERSION {
        write(&world.join("level.dat"), LEGACY_LEVEL)?;
    } else {
        return Err(Error::Launch(
            "这个版本太老，测试世界打不开（需要 1.20.1 及以上）".into(),
        ));
    }
    let pack = world.join("datapacks").join("nailong-bench");
    write(
        &pack.join("pack.mcmeta"),
        format!(
            r#"{{"pack":{{"pack_format":{},"description":"nailong benchmark"}}}}"#,
            info.pack_format
        )
        .as_bytes(),
    )?;
    let functions = [
        (
            "load",
            "scoreboard objectives add nailong dummy\n".to_string(),
        ),
        (
            "join",
            "gamemode spectator @s\ntag @s add nailong\nsay nailong-bench:joined\n".to_string(),
        ),
        ("tick", tick_function()),
    ];
    // 1.21 起目录名是单数 function，之前是 functions；两个都写，旧版本会忽略不认识的目录。
    for dir in ["function", "functions"] {
        for (name, body) in &functions {
            write(
                &pack
                    .join("data/nailong")
                    .join(dir)
                    .join(format!("{name}.mcfunction")),
                body.as_bytes(),
            )?;
        }
        for tag in ["tick", "load"] {
            write(
                &pack
                    .join("data/minecraft/tags")
                    .join(dir)
                    .join(format!("{tag}.json")),
                format!(r#"{{"values":["nailong:{tag}"]}}"#).as_bytes(),
            )?;
        }
    }
    Ok(())
}

fn tick_function() -> String {
    let end = WARMUP_TICKS + ROUTE_TICKS;
    format!(
        "execute as @a[tag=!nailong] run function nailong:join\n\
         execute if entity @a[tag=nailong] run scoreboard players add #t nailong 1\n\
         execute if score #t nailong matches ..{end} as @a[tag=nailong] at @s run tp @s ~ 140 ~1 0 25\n\
         execute if score #t nailong matches {WARMUP_TICKS} run say nailong-bench:start\n\
         execute if score #t nailong matches {end} run say nailong-bench:done\n"
    )
}

/// 这些目录决定游戏要加载什么；存档、日志、地图缓存这类大目录不复制。
const INSTANCE_DIRS: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "kubejs",
    "resourcepacks",
    "shaderpacks",
    "scripts",
    "global_packs",
    "openloader",
    "paxi",
    "fancymenu_data",
];

/// 把版本的游戏目录复制成测试实例。jar 和 zip 游戏只读不写，用硬链接；
/// 其他文件游戏可能原地改写，必须真复制，免得改到用户自己的文件。
fn prepare_instance(source: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| io(dest, e))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| io(dest, e))?;
    for dir in INSTANCE_DIRS {
        let from = source.join(dir);
        if from.is_dir() {
            copy_tree(&from, &dest.join(dir))?;
        }
    }
    let options = std::fs::read_to_string(source.join("options.txt")).unwrap_or_default();
    write(
        &dest.join("options.txt"),
        bench_options(&options).as_bytes(),
    )
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| io(from, e))?.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            copy_tree(&path, &target)?;
        } else if kind.is_file() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let linked = (name.ends_with(".jar") || name.ends_with(".zip"))
                && std::fs::hard_link(&path, &target).is_ok();
            if !linked {
                std::fs::copy(&path, &target).map_err(|e| io(&path, e))?;
            }
        }
    }
    Ok(())
}

/// 保留用户自己的画面设置（视距会影响内存），关掉会挡住测试的提示和声音。
fn bench_options(original: &str) -> String {
    const FORCED: &[(&str, &str)] = &[
        ("pauseOnLostFocus", "false"),
        ("onboardAccessibility", "false"),
        ("tutorialStep", "none"),
        ("skipMultiplayerWarning", "true"),
        ("joinedFirstServer", "true"),
        ("narrator", "0"),
        ("fullscreen", "false"),
        ("soundCategory_master", "0.0"),
    ];
    let mut lines: Vec<String> = original
        .lines()
        .filter(|line| {
            let key = line.split(':').next().unwrap_or("");
            !FORCED.iter().any(|(forced, _)| *forced == key)
        })
        .map(String::from)
        .collect();
    lines.extend(FORCED.iter().map(|(k, v)| format!("{k}:{v}")));
    lines.join("\n") + "\n"
}

/// GC 日志里的一条记录。
#[derive(Debug, Clone, PartialEq)]
struct GcEvent {
    /// JVM 启动后的秒数。
    at: f64,
    /// 停顿时长；ZGC 的回收周期行没有停顿，为 None。
    pause_ms: Option<f64>,
    /// 回收后的堆占用。
    after_mb: Option<u32>,
    /// 这次回收是否清理了老年代：G1 的混合/整堆回收、ZGC 的大回收。
    old: bool,
    full: bool,
    evac_failure: bool,
}

fn parse_gc_log(text: &str) -> Vec<GcEvent> {
    text.lines().filter_map(parse_gc_line).collect()
}

fn parse_gc_line(line: &str) -> Option<GcEvent> {
    let rest = line.strip_prefix('[')?;
    let (uptime, rest) = rest.split_once("s]")?;
    let at: f64 = uptime.parse().ok()?;
    let body = rest.trim();
    let gc_pos = body.find("GC(")?;
    let body = &body[gc_pos..];
    let pause = body.contains("Pause ");
    let collection = body.contains("Collection (") || body.contains("Garbage Collection");
    if !pause && !collection {
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
    if pause_ms.is_none() && after_mb.is_none() {
        return None;
    }
    let full = body.contains("Pause Full");
    Some(GcEvent {
        at,
        pause_ms,
        after_mb,
        old: full || body.contains("(Mixed)") || body.contains("Major Collection"),
        full,
        evac_failure: body.contains("Evacuation Failure") || body.contains("To-space exhausted"),
    })
}

fn summarize(events: &[GcEvent], from: f64, to: f64) -> GcSummary {
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

/// 存活数据：清理过老年代的回收之后剩下的；没有这类回收时退而取所有回收后的最大值（偏大）。
fn live_set(events: &[GcEvent]) -> Option<u32> {
    let after_old = events
        .iter()
        .filter(|e| e.old)
        .filter_map(|e| e.after_mb)
        .max();
    after_old.or_else(|| events.iter().filter_map(|e| e.after_mb).max())
}

/// 进程从启动起用掉的 CPU 秒数。
#[cfg(target_os = "linux")]
fn cpu_secs(child: &std::process::Child) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", child.id())).ok()?;
    // 进程名在括号里，可能含空格，从右括号之后开始数字段。
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    // Linux 的 USER_HZ 在所有主流架构上都是 100。
    Some((utime + stime) / 100.0)
}

#[cfg(windows)]
fn cpu_secs(child: &std::process::Child) -> Option<f64> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = Default::default();
    // SAFETY: 句柄来自仍在运行的子进程，四个输出参数都是有效的 FILETIME。
    let ok = unsafe { GetProcessTimes(child.as_raw_handle(), &mut c, &mut e, &mut k, &mut u) };
    if ok == 0 {
        return None;
    }
    let secs = |t: &FileTime| ((t.high as u64) << 32 | t.low as u64) as f64 / 10_000_000.0;
    Some(secs(&k) + secs(&u))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn cpu_secs(_child: &std::process::Child) -> Option<f64> {
    None
}

/// 一轮测试怎么跑。
struct RunSpec {
    label: &'static str,
    collector: Collector,
    heap_mb: u32,
    /// 测量轮额外的参数：让 G1 频繁做并发标记，回收后的占用更接近真实存活数据。
    extra: Vec<String>,
}

/// 进度：每轮占总进度的一份，加载占一轮的 30%，飞行占 70%。
struct Meter<'a> {
    sink: &'a Sink,
    round: usize,
    rounds: usize,
}

impl Meter<'_> {
    fn report(&self, stage: String, within: f32) {
        let done = (self.round as f32 + within.clamp(0.0, 1.0)) / self.rounds as f32;
        (self.sink)(Progress {
            stage,
            files_done: (done * 1000.0) as u64,
            files_total: 1000,
            ..Default::default()
        });
    }
}

enum Line {
    Text(String),
    Closed,
}

fn run_once(
    root: &Root,
    profile: &str,
    dest: &Path,
    spec: &RunSpec,
    meter: &Meter,
) -> Result<RunReport> {
    cancel::check()?;
    let gc_log = dest.join(GC_LOG);
    let _ = std::fs::remove_file(&gc_log);
    let mut jvm_args = spec.extra.clone();
    jvm_args.push(format!("-Xlog:gc,gc+phases:file={GC_LOG}:uptime"));
    let opts = LaunchOptions {
        memory_mb: Some(spec.heap_mb),
        gc: match spec.collector {
            Collector::G1 => GcChoice::G1,
            Collector::Zgc => GcChoice::Zgc,
        },
        jvm_args,
        game_dir: Some(dest.to_path_buf()),
        ..Default::default()
    };
    let mut prepared = launch::prepare(root, profile, &opts)?;
    prepared
        .game
        .extend(["--quickPlaySingleplayer".to_string(), WORLD.to_string()]);
    let mut cmd = prepared.command();
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Launch(format!("无法启动 Java: {e}")))?;

    let (tx, rx) = mpsc::channel();
    let mut readers = Vec::new();
    let stdout = child
        .stdout
        .take()
        .map(|s| Box::new(s) as Box<dyn Read + Send>);
    let stderr = child
        .stderr
        .take()
        .map(|s| Box::new(s) as Box<dyn Read + Send>);
    for stream in [stdout, stderr].into_iter().flatten() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                if tx.send(Line::Text(line)).is_err() {
                    break;
                }
            }
            let _ = tx.send(Line::Closed);
        }));
    }
    drop(tx);
    let log_path = dest.join(GAME_LOG);
    let mut log = std::fs::File::create(&log_path).map_err(|e| io(&log_path, e))?;

    let route_budget = Duration::from_secs((ROUTE_TICKS / 20 * 4 + 60) as u64);
    let (mut joined, mut start, mut done) = (None, None, None);
    let (mut cpu_start, mut cpu_done) = (None, None);
    let mut closed = 0;
    let mut tail: Vec<String> = Vec::new();
    let outcome: Result<()> = loop {
        if cancel::is_cancelled() {
            break Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Line::Text(line)) => {
                let _ = writeln!(log, "{}", launch::redact(&line));
                if tail.len() >= 40 {
                    tail.remove(0);
                }
                tail.push(line.clone());
                let at = started.elapsed();
                if joined.is_none() && line.contains("nailong-bench:joined") {
                    joined = Some(at);
                } else if start.is_none() && line.contains("nailong-bench:start") {
                    start = Some(at);
                    cpu_start = cpu_secs(&child);
                } else if line.contains("nailong-bench:done") {
                    done = Some(at);
                    cpu_done = cpu_secs(&child);
                    break Ok(());
                }
            }
            Ok(Line::Closed) => {
                closed += 1;
                if closed == 2 {
                    break Err(Error::Launch(exit_reason(&tail)));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err(Error::Launch(exit_reason(&tail)));
            }
        }
        let elapsed = started.elapsed();
        match start {
            None if elapsed > JOIN_TIMEOUT => {
                break Err(Error::Launch(
                    "等了 8 分钟还没进入测试世界，可能停在了某个提示界面".into(),
                ));
            }
            None => meter.report(
                format!("{}：加载游戏", spec.label),
                0.3 * (elapsed.as_secs_f32() / 90.0).min(0.95),
            ),
            Some(at) if elapsed - at > route_budget => {
                break Err(Error::Launch("测试世界跑得太慢，超过了时间上限".into()));
            }
            Some(at) => {
                let share = (elapsed - at).as_secs_f32() / (ROUTE_TICKS as f32 / 20.0);
                meter.report(
                    format!("{}：飞行测试", spec.label),
                    0.3 + 0.7 * share.min(1.0),
                );
            }
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
    outcome?;

    let (Some(joined), Some(start), Some(done)) = (joined, start, done) else {
        return Err(Error::Launch("测试没有完整跑完".into()));
    };
    let events = std::fs::read_to_string(&gc_log)
        .map(|text| parse_gc_log(&text))
        .unwrap_or_default();
    let route = (done - start).as_secs_f64();
    let cpu_cores = match (cpu_start, cpu_done) {
        (Some(a), Some(b)) if route > 0.0 => Some((b - a) / route),
        _ => None,
    };
    Ok(RunReport {
        label: spec.label.to_string(),
        collector: spec.collector,
        heap_mb: spec.heap_mb,
        load_secs: joined.as_secs_f64(),
        tps: ROUTE_TICKS as f64 / route.max(0.001),
        cpu_cores,
        gc: summarize(&events, start.as_secs_f64(), done.as_secs_f64()),
        live_mb: live_set(&events),
    })
}

/// 游戏提前退出时，从最后几行输出里找出原因。
fn exit_reason(tail: &[String]) -> String {
    let hint = tail
        .iter()
        .rev()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("exception") || lower.contains("error") || lower.contains("crash")
        })
        .map(|line| line.trim().chars().take(160).collect::<String>());
    match hint {
        Some(hint) => format!("游戏在进入测试世界前退出了：{hint}"),
        None => "游戏在进入测试世界前退出了".into(),
    }
}

/// G1 需要的堆：存活数据占 60%，至少比存活多 1 GB。
fn g1_heap(live: u32, cap: u32) -> u32 {
    let need = ((live as f32 / G1_LIVE_SHARE) as u32).max(live + 1024);
    tuning_round(need).clamp(2048.min(cap), cap)
}

/// ZGC 需要的堆：存活数据占一半，且不少于 G1 的 1.25 倍。
fn zgc_heap(live: u32, g1: u32, cap: u32) -> Option<u32> {
    let need = ((live as f32 / ZGC_LIVE_SHARE) as u32).max(g1 + g1 / 4);
    let heap = tuning_round(need);
    (heap <= cap).then_some(heap)
}

fn tuning_round(mb: u32) -> u32 {
    mb.div_ceil(512) * 512
}

/// 比较 G1 和 ZGC 两轮，决定用哪个，并写下理由。
fn decide(
    g1: &RunReport,
    zgc: Option<&RunReport>,
    official_zgc: bool,
    threads: usize,
    notes: &mut Vec<String>,
) -> Collector {
    let Some(zgc) = zgc else {
        notes.push("没有跑 ZGC 对比，用 G1".into());
        return Collector::G1;
    };
    let minutes = (ROUTE_TICKS as f64 / 20.0 / 60.0).max(0.1);
    let stutter = g1.gc.over_frame as f64 / minutes;
    let extra_cores = match (zgc.cpu_cores, g1.cpu_cores) {
        (Some(z), Some(g)) => Some(z - g),
        _ => None,
    };
    let budget = threads as f64 * if official_zgc { 0.2 } else { 0.1 };
    let cpu_ok = extra_cores.is_none_or(|extra| extra <= budget);
    let slower = zgc.tps < g1.tps * if official_zgc { 0.95 } else { 0.97 };
    if let Some(extra) = extra_cores {
        notes.push(format!("ZGC 平均多用 {extra:.1} 个核（上限 {budget:.1}）"));
    }
    notes.push(format!(
        "G1 每分钟 {stutter:.1} 次超过一帧的停顿，最长 {:.0} ms；ZGC 最长 {:.1} ms",
        g1.gc.max_ms, zgc.gc.max_ms
    ));
    if slower {
        notes.push(format!(
            "ZGC 的服务端速度 {:.1} tick/s 低于 G1 的 {:.1}",
            zgc.tps, g1.tps
        ));
    }
    let choose_zgc = if official_zgc {
        // 官方默认就是 ZGC：除非它明显拖慢或太吃 CPU，否则沿用。
        cpu_ok && !slower
    } else {
        // 一分钟里偶尔一次刚过一帧的停顿是噪声，至少两次或有明显长停顿才算卡。
        cpu_ok
            && !slower
            && (stutter >= 2.0 || g1.gc.max_ms >= 30.0)
            && zgc.gc.max_ms < g1.gc.max_ms
    };
    notes.push(if choose_zgc {
        "结论：ZGC".into()
    } else {
        "结论：G1".into()
    });
    if choose_zgc {
        Collector::Zgc
    } else {
        Collector::G1
    }
}

/// 对一个版本跑完整的实测，保存并返回结果。
pub fn run(root: &Root, profile: &str, game_dir: &Path, sink: &Sink) -> Result<Record> {
    cancel::check()?;
    // 和启动时一样用绝对路径，结果才能按同一个键找回来。
    let game_dir = &std::path::absolute(game_dir).unwrap_or_else(|_| game_dir.to_path_buf());
    let machine = Machine::detect();
    (sink)(Progress::stage("准备测试实例"));
    let probe = launch::prepare(
        root,
        profile,
        &LaunchOptions {
            game_dir: Some(game_dir.to_path_buf()),
            ..Default::default()
        },
    )?;
    if !probe.supports_quick_play(root) {
        return Err(Error::Launch(
            "1.20 以前的版本不能自动进入世界，没法实测，仍按公式估算".into(),
        ));
    }
    let info = game_info(&probe.client_jar)?;
    let java = probe.java.clone();
    let cap = probe.plan.cap_mb.unwrap_or(4096);
    let official_zgc = probe.plan.official_collector == Some(Collector::Zgc);
    let zgc_ok = java.major >= 21 && java.is_64bit() && !java.is_graal();

    let dest = root.cache().join("bench").join(sanitize(profile));
    prepare_instance(game_dir, &dest)?;
    prepare_world(&dest, info)?;

    let rounds = if zgc_ok { 3 } else { 2 };
    let mut meter = Meter {
        sink,
        round: 0,
        rounds,
    };
    let mut runs = Vec::new();
    let mut notes = Vec::new();

    // 第 1 轮：堆给足，让 G1 频繁做并发标记，量出真实的存活数据。
    let measure_heap = tuning_round(probe.plan.need_mb * 3 / 2).clamp(2048.min(cap), cap);
    let measure = run_once(
        root,
        profile,
        &dest,
        &RunSpec {
            label: "第 1 轮 · 测量内存",
            collector: Collector::G1,
            heap_mb: measure_heap,
            extra: vec![
                "-XX:-G1UseAdaptiveIHOP".into(),
                "-XX:InitiatingHeapOccupancyPercent=20".into(),
            ],
        },
        &meter,
    )?;
    let live = measure.live_mb.unwrap_or(probe.plan.need_mb / 2).max(256);
    runs.push(measure);
    let mut g1 = g1_heap(live, cap);
    notes.push(format!(
        "存活数据最多 {}，G1 按 60% 占用算出 {}",
        gb(live),
        gb(g1)
    ));
    if g1 >= cap && (live as f32 / G1_LIVE_SHARE) as u32 > cap {
        notes.push(format!("电脑内存只够给 {}，这个版本会比较吃紧", gb(cap)));
    }

    // 第 2 轮：用算出来的堆和正式参数验证；有整堆回收或晋升失败就加 1/4 再跑一次。
    meter.round = 1;
    let mut check = run_once(
        root,
        profile,
        &dest,
        &RunSpec {
            label: "第 2 轮 · 验证 G1",
            collector: Collector::G1,
            heap_mb: g1,
            extra: Vec::new(),
        },
        &meter,
    )?;
    if (check.gc.full > 0 || check.gc.evac_failures > 0) && g1 < cap {
        notes.push(format!(
            "{} 下出现 {} 次整堆回收、{} 次晋升失败，加大再测",
            gb(g1),
            check.gc.full,
            check.gc.evac_failures
        ));
        g1 = tuning_round(g1 + g1 / 4).min(cap);
        runs.push(check);
        check = run_once(
            root,
            profile,
            &dest,
            &RunSpec {
                label: "第 2 轮 · 加大后验证 G1",
                collector: Collector::G1,
                heap_mb: g1,
                extra: Vec::new(),
            },
            &meter,
        )?;
    }
    runs.push(check);

    // 第 3 轮：ZGC 对比。
    let mut zgc_heap_mb = None;
    let mut zgc_run = None;
    if zgc_ok {
        meter.round = 2;
        match zgc_heap(live, g1, cap) {
            Some(heap) => {
                zgc_heap_mb = Some(heap);
                let report = run_once(
                    root,
                    profile,
                    &dest,
                    &RunSpec {
                        label: "第 3 轮 · 对比 ZGC",
                        collector: Collector::Zgc,
                        heap_mb: heap,
                        extra: Vec::new(),
                    },
                    &meter,
                )?;
                zgc_run = Some(runs.len());
                runs.push(report);
            }
            None => notes.push("内存不够 ZGC 需要的余量，跳过 ZGC 对比".into()),
        }
    } else {
        notes.push(format!(
            "Java {}{} 不适合 ZGC，跳过对比",
            java.major,
            if java.is_graal() { "（GraalVM）" } else { "" }
        ));
    }
    let g1_run = runs
        .iter()
        .rev()
        .find(|r| r.collector == Collector::G1)
        .cloned()
        .expect("G1 验证轮一定存在");
    let collector = decide(
        &g1_run,
        zgc_run.map(|i| &runs[i]),
        official_zgc,
        machine.threads,
        &mut notes,
    );

    let record = Record {
        profile: profile.to_string(),
        game_dir: game_dir.to_path_buf(),
        fingerprint: fingerprint(profile, game_dir, java.major),
        when: now(),
        java_major: java.major,
        live_mb: live,
        g1_heap_mb: g1,
        zgc_heap_mb,
        collector,
        runs,
        notes,
    };
    save(root, &record)?;
    let _ = std::fs::remove_dir_all(dest.join("saves"));
    (sink)(Progress {
        stage: "实测完成".into(),
        files_done: 1000,
        files_total: 1000,
        ..Default::default()
    });
    Ok(record)
}

fn sanitize(profile: &str) -> String {
    profile
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pack_and_world_versions() {
        let v120 = r#"{"world_version": 3465, "pack_version": {"resource": 15, "data": 15}}"#;
        let v263 =
            r#"{"world_version": 5023, "pack_version": {"resource_major": 97, "data_major": 121}}"#;
        assert_eq!(
            parse_game_info(v120),
            Some(GameInfo {
                world_version: 3465,
                pack_format: 15
            })
        );
        assert_eq!(parse_game_info(v263).unwrap().pack_format, 121);
        assert_eq!(parse_game_info(r#"{"pack_version": 9}"#), None);
    }

    #[test]
    fn patches_the_data_version_of_the_26_template() {
        let patched = patch_data_version(V26_FILES[0].1, 5023).unwrap();
        let mut data = Vec::new();
        flate2::read::GzDecoder::new(&patched[..])
            .read_to_end(&mut data)
            .unwrap();
        let pattern = b"\x03\x00\x0bDataVersion";
        let at = find(&data, pattern).unwrap() + pattern.len();
        assert_eq!(
            i32::from_be_bytes(data[at..at + 4].try_into().unwrap()),
            5023
        );
    }

    #[test]
    fn writes_a_world_for_each_era() {
        let dir = std::env::temp_dir().join(format!("nailong-bench-test-{}", std::process::id()));
        for (version, pack, expect_data_dir) in [(3955, 48, false), (5023, 121, true)] {
            prepare_world(
                &dir,
                GameInfo {
                    world_version: version,
                    pack_format: pack,
                },
            )
            .unwrap();
            let world = dir.join("saves").join(WORLD);
            assert!(world.join("level.dat").exists());
            assert_eq!(
                world.join("data/minecraft/world_gen_settings.dat").exists(),
                expect_data_dir
            );
            let mcmeta =
                std::fs::read_to_string(world.join("datapacks/nailong-bench/pack.mcmeta")).unwrap();
            assert!(mcmeta.contains(&format!("\"pack_format\":{pack}")));
            for dir in ["function", "functions"] {
                assert!(
                    world
                        .join("datapacks/nailong-bench/data/nailong")
                        .join(dir)
                        .join("tick.mcfunction")
                        .exists()
                );
            }
        }
        assert!(
            prepare_world(
                &dir,
                GameInfo {
                    world_version: 3100,
                    pack_format: 10
                }
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_user_options_but_forces_the_test_ones() {
        let out = bench_options("renderDistance:16\npauseOnLostFocus:true\nfov:0.5");
        assert!(out.contains("renderDistance:16"));
        assert!(out.contains("pauseOnLostFocus:false"));
        assert!(!out.contains("pauseOnLostFocus:true"));
        assert!(out.contains("soundCategory_master:0.0"));
    }

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
        let events = parse_gc_log(log);
        assert_eq!(events.len(), 8);
        let route = summarize(&events, 30.0, 45.0);
        assert_eq!(route.pauses, 5);
        assert_eq!(route.over_frame, 3);
        assert_eq!(route.full, 1);
        assert_eq!(route.evac_failures, 1);
        assert!((route.max_ms - 400.0).abs() < 1e-9);
        // 清理过老年代的回收后最大：整堆回收后的 1200M。
        assert_eq!(live_set(&events), Some(1200));
        assert_eq!(live_set(&events[..2]), Some(300));
    }

    #[test]
    fn sizes_heaps_from_the_live_set() {
        assert_eq!(g1_heap(900, 11264), 2048);
        // 3000 / 0.6 = 5000 → 5120。
        assert_eq!(g1_heap(3000, 11264), 5120);
        assert_eq!(g1_heap(9000, 11264), 11264);
        assert_eq!(zgc_heap(3000, 5120, 11264), Some(6656));
        assert_eq!(zgc_heap(6000, 10240, 11264), None);
    }

    fn report(
        collector: Collector,
        tps: f64,
        cpu: f64,
        max_ms: f64,
        over_frame: usize,
    ) -> RunReport {
        RunReport {
            label: String::new(),
            collector,
            heap_mb: 4096,
            load_secs: 30.0,
            tps,
            cpu_cores: Some(cpu),
            gc: GcSummary {
                max_ms,
                over_frame,
                ..Default::default()
            },
            live_mb: Some(1000),
        }
    }

    #[test]
    fn zgc_has_to_earn_its_place() {
        let mut notes = Vec::new();
        let smooth_g1 = report(Collector::G1, 20.0, 3.0, 12.0, 0);
        let stutter_g1 = report(Collector::G1, 20.0, 3.0, 45.0, 4);
        let borderline_g1 = report(Collector::G1, 20.0, 3.0, 17.2, 1);
        let zgc = report(Collector::Zgc, 20.0, 3.8, 0.1, 0);
        let hungry_zgc = report(Collector::Zgc, 20.0, 6.0, 0.1, 0);
        // G1 已经很顺：留 G1。
        assert_eq!(
            decide(&smooth_g1, Some(&zgc), false, 16, &mut notes),
            Collector::G1
        );
        // 一分钟只有一次刚过一帧的停顿：算噪声，留 G1。
        assert_eq!(
            decide(&borderline_g1, Some(&zgc), false, 16, &mut notes),
            Collector::G1
        );
        // G1 卡顿、ZGC 多用的 CPU 在 16 线程的 10% 以内：换 ZGC。
        assert_eq!(
            decide(&stutter_g1, Some(&zgc), false, 16, &mut notes),
            Collector::Zgc
        );
        // ZGC 太吃 CPU：留 G1。
        assert_eq!(
            decide(&stutter_g1, Some(&hungry_zgc), false, 16, &mut notes),
            Collector::G1
        );
        // 官方默认 ZGC 时，只要不明显拖慢就沿用。
        assert_eq!(
            decide(&smooth_g1, Some(&zgc), true, 16, &mut notes),
            Collector::Zgc
        );
        assert_eq!(
            decide(&smooth_g1, None, true, 16, &mut notes),
            Collector::G1
        );
    }
}
