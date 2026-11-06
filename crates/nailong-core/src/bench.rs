//! 通用调参测试：不开游戏、不碰任何版本，用一个内置的小 Java 程序模拟
//! Minecraft 的内存和线程特征，在这台电脑的每个 Java 21+ 上比较 G1 和 ZGC。
//!
//! 结果按「这台电脑 + 这个 Java」存一份，所有版本共用，只决定选哪个回收器。
//! 堆大小不在这里测：它取决于装了哪些 Mod，由 `learned.rs` 从真实游戏里学。
//! 规范见 `docs/design/20-tuning-benchmark.md`，负载程序见 `assets/jvmbench/`。

use crate::cancel;
use crate::error::{Error, Result, io};
use crate::gclog::{self, GcSummary};
use crate::java::{self, JavaRuntime};
use crate::paths::Root;
use crate::progress::{Progress, Sink};
use crate::tuning::{
    self, BenchVerdict, Collector, GcChoice, Loader, Machine, Settings, TierVerdict, Workload, gb,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 负载程序，Java 8 字节码，所有 Java 都能跑。源码在同一目录。
const CLASS: &[u8] = include_bytes!("../assets/jvmbench/NailongBench.class");
const CLASS_NAME: &str = "NailongBench";
const STORE: &str = "jvm-bench.json";
const GC_LOG: &str = "gc.log";
/// 每轮测量的秒数。
pub const MEASURE_SECS: u32 = 12;
/// 服务端一个 tick 的预算。
const TICK_MS: f64 = 50.0;
/// 建数据最多等这么久：大档要在堆里放 3 GB。
const READY_TIMEOUT: Duration = Duration::from_secs(120);

/// 一档负载。常驻数据 + 区块缓存约等于存活数据。
#[derive(Debug, Clone, Copy)]
pub struct Tier {
    pub key: &'static str,
    pub label: &'static str,
    world_mb: u32,
    cache_mb: u32,
    chunks_per_sec: u32,
    tick_garbage_mb: u32,
    workers: usize,
    warmup_secs: u32,
}

impl Tier {
    pub fn live_mb(&self) -> u32 {
        self.world_mb + self.cache_mb
    }
}

/// 常规：原版或轻量 Mod，存活约 1 GB。大型整合包：存活约 3 GB，生成更快、垃圾更多。
pub const TIERS: [Tier; 2] = [
    Tier {
        key: "standard",
        label: "常规",
        world_mb: 700,
        cache_mb: 300,
        chunks_per_sec: 120,
        tick_garbage_mb: 15,
        workers: 2,
        warmup_secs: 3,
    },
    Tier {
        key: "large",
        label: "大型整合包",
        world_mb: 2200,
        cache_mb: 800,
        chunks_per_sec: 200,
        tick_garbage_mb: 25,
        workers: 3,
        warmup_secs: 4,
    },
];

/// 一轮的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub collector: Collector,
    pub heap_mb: u32,
    /// 模拟渲染的平均帧率。负载是固定的，所以只用来横向比较，不是游戏帧率。
    pub fps: f64,
    /// 1% 低帧：最慢 1% 的帧换算成的帧率。
    pub low_fps: f64,
    pub frame_max_ms: f64,
    /// 每分钟能看出来的顿（比平时多出半帧以上）。
    pub hitches_per_min: f64,
    pub tick_p95_ms: f64,
    pub tick_max_ms: f64,
    /// 实际生成的区块 ÷ 目标，满速是 1。
    pub chunk_ratio: f64,
    /// 测量段平均占用的 CPU 核数；读不到时为 None。
    pub cpu_cores: Option<f64>,
    pub gc: GcSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierResult {
    pub key: String,
    pub label: String,
    pub live_mb: u32,
    pub g1: RunReport,
    pub zgc: RunReport,
    /// 一般版本的结论。
    pub collector: Collector,
    /// 官方默认 ZGC 的版本（26.1 起）的结论。
    pub official: Collector,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaResult {
    pub path: PathBuf,
    pub major: u32,
    pub version: String,
    pub vendor: Option<String>,
    pub tiers: Vec<TierResult>,
    pub notes: Vec<String>,
}

impl JavaResult {
    pub fn label(&self) -> String {
        match &self.vendor {
            Some(vendor) => format!("Java {} ({vendor})", self.version),
            None => format!("Java {}", self.version),
        }
    }

    pub fn verdict(&self, when: u64) -> Option<BenchVerdict> {
        let pick = |key: &str| {
            self.tiers
                .iter()
                .find(|tier| tier.key == key)
                .map(|tier| TierVerdict {
                    collector: tier.collector,
                    official: tier.official,
                })
        };
        Some(BenchVerdict {
            standard: pick("standard")?,
            large: pick("large"),
            age: age(when),
        })
    }
}

/// 存进 `jvm-bench.json` 的一次测试。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    /// Unix 秒。
    pub when: u64,
    /// 测试时的线程数和内存（整 GB）。变了就说明换了电脑或内存条，结果不再作数。
    pub threads: usize,
    pub total_gb: u32,
    pub javas: Vec<JavaResult>,
    /// 没测的 Java 和原因。
    pub skipped: Vec<String>,
}

impl Store {
    /// 还是这台电脑测的。
    pub fn fresh(&self) -> bool {
        let machine = Machine::detect();
        machine.threads == self.threads && total_gb(&machine) == self.total_gb
    }

    /// 这个 Java 的结论：先找同一个程序，再找同一个大版本。
    pub fn find(&self, java: &JavaRuntime) -> Option<&JavaResult> {
        let same = |a: &Path, b: &Path| {
            a == b
                || std::fs::canonicalize(a)
                    .ok()
                    .zip(std::fs::canonicalize(b).ok())
                    .is_some_and(|(a, b)| a == b)
        };
        self.javas
            .iter()
            .find(|result| same(&result.path, &java.path))
            .or_else(|| self.javas.iter().find(|result| result.major == java.major))
    }

    pub fn verdict(&self, java: &JavaRuntime) -> Option<BenchVerdict> {
        if java.is_graal() || !java.is_64bit() || !self.fresh() {
            return None;
        }
        self.find(java)?.verdict(self.when)
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

fn total_gb(machine: &Machine) -> u32 {
    machine.total_mb.unwrap_or(0).div_ceil(1024)
}

fn store_path(root: &Root) -> PathBuf {
    root.dir().join(STORE)
}

/// 读出上次的测试。结论按现在的规则从测到的数字重新判，以后调门槛不用重测。
pub fn load(root: &Root) -> Option<Store> {
    let mut store: Store = std::fs::read(store_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
    let threads = store.threads;
    for java in &mut store.javas {
        for tier in &mut java.tiers {
            let mut notes = Vec::new();
            (tier.collector, tier.official) = decide(&tier.g1, &tier.zgc, threads, &mut notes);
            tier.notes = notes;
        }
    }
    Some(store)
}

fn save(root: &Root, store: &Store) -> Result<()> {
    let path = store_path(root);
    let bytes = serde_json::to_vec_pretty(store).map_err(|e| Error::Launch(e.to_string()))?;
    std::fs::write(&path, bytes).map_err(|e| io(&path, e))
}

/// 这个 Java 启动游戏时用的测试结论；没测过、换了电脑或不适合 ZGC 时为 None。
pub fn verdict(root: &Root, java: &JavaRuntime) -> Option<BenchVerdict> {
    load(root)?.verdict(java)
}

/// 0.35 给每个版本建的测试实例和结果，现在用不到了。
pub fn cleanup_legacy(root: &Root) {
    let _ = std::fs::remove_file(root.dir().join("bench.json"));
    let _ = std::fs::remove_dir_all(root.cache().join("bench"));
}

/// 要测的 Java：64 位、21 起（分代 ZGC）、不是 GraalVM（它用 ZGC 时会关掉自己的编译器），
/// 每个大版本只测启动游戏时会选中的那一个。
pub fn candidates(javas: &[JavaRuntime]) -> (Vec<JavaRuntime>, Vec<String>) {
    let mut picked: Vec<JavaRuntime> = Vec::new();
    let mut skipped = Vec::new();
    for java in javas {
        if picked.iter().any(|p| p.major == java.major) {
            continue;
        }
        if java.major < 21 {
            continue;
        }
        if let Ok(choice) = java::select(java.major, javas)
            && choice.major == java.major
            && choice.is_64bit()
            && !choice.is_graal()
        {
            picked.push(choice);
        } else if !javas
            .iter()
            .any(|other| other.major == java.major && other.is_64bit() && !other.is_graal())
        {
            skipped.push(format!(
                "{}：{}，只用 G1",
                java.label(),
                if java.is_graal() {
                    "GraalVM 用 ZGC 会关掉自己的编译器"
                } else {
                    "32 位不支持 ZGC"
                }
            ));
        }
    }
    picked.sort_by_key(|java| java.major);
    (picked, skipped)
}

/// 和正式启动一样的回收器参数，堆固定为 `heap_mb`。
fn jvm_args(java: &JavaRuntime, collector: Collector, heap_mb: u32) -> Vec<String> {
    let work = Workload {
        java_major: java.major,
        java_64bit: true,
        graal: false,
        loader: Loader::Vanilla,
        mods: 0,
        official: Vec::new(),
    };
    let settings = Settings {
        memory_mb: Some(heap_mb),
        min_memory_mb: Some(heap_mb),
        gc: match collector {
            Collector::G1 => GcChoice::G1,
            Collector::Zgc => GcChoice::Zgc,
        },
        ..Default::default()
    };
    let mut args = tuning::plan(&Machine::detect(), &work, &settings).args();
    // 预先占好整个堆：只测十几秒，第一次碰到新内存的缺页会盖过回收器本身的差别。
    args.push("-XX:+AlwaysPreTouch".into());
    args.push(format!(
        "-Xlog:gc,gc+phases:file={GC_LOG}:uptime:filecount=0"
    ));
    args
}

#[derive(Debug, Default, PartialEq)]
struct Output {
    secs: f64,
    frames: u64,
    frame_p50: f64,
    frame_p99: f64,
    frame_max: f64,
    hitches: u64,
    tick_p95: f64,
    tick_max: f64,
    chunks: f64,
    chunk_target: f64,
}

fn parse_result(line: &str) -> Option<Output> {
    let rest = line.trim().strip_prefix("nailong-bench:result")?;
    let mut out = Output::default();
    for pair in rest.split_whitespace() {
        let (key, value) = pair.split_once('=')?;
        let value: f64 = value.parse().ok()?;
        match key {
            "secs" => out.secs = value,
            "frames" => out.frames = value as u64,
            "frame_p50" => out.frame_p50 = value,
            "frame_p99" => out.frame_p99 = value,
            "frame_max" => out.frame_max = value,
            "hitches" => out.hitches = value as u64,
            "tick_p95" => out.tick_p95 = value,
            "tick_max" => out.tick_max = value,
            "chunks" => out.chunks = value,
            "chunk_target" => out.chunk_target = value,
            _ => {}
        }
    }
    (out.secs > 0.0 && out.frames > 0).then_some(out)
}

/// 进度：每轮占一份；建数据和预热占一轮的 35%，测量占 65%。
struct Meter<'a> {
    sink: &'a Sink,
    round: usize,
    rounds: usize,
}

impl Meter<'_> {
    fn report(&self, stage: String, within: f32) {
        let done = (self.round as f32 + within.clamp(0.0, 1.0)) / self.rounds.max(1) as f32;
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

fn run_once(
    dir: &Path,
    java: &JavaRuntime,
    tier: &Tier,
    collector: Collector,
    heap_mb: u32,
    stage: &str,
    meter: &Meter,
) -> Result<RunReport> {
    cancel::check()?;
    let gc_log = dir.join(GC_LOG);
    let _ = std::fs::remove_file(&gc_log);
    let threads = Machine::detect().threads;
    let workers = tier.workers.min((threads / 4).max(1));
    let mut cmd = Command::new(&java.path);
    cmd.args(jvm_args(java, collector, heap_mb))
        .args(["-cp", ".", CLASS_NAME])
        .args(
            [
                tier.warmup_secs,
                MEASURE_SECS,
                tier.world_mb,
                tier.cache_mb,
                tier.chunks_per_sec,
                tier.tick_garbage_mb,
                workers as u32,
            ]
            .map(|n| n.to_string()),
        )
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for var in ["_JAVA_OPTIONS", "JAVA_TOOL_OPTIONS", "JDK_JAVA_OPTIONS"] {
        cmd.env_remove(var);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
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

    let budget = READY_TIMEOUT + Duration::from_secs((tier.warmup_secs + MEASURE_SECS + 30) as u64);
    let (mut ready, mut start, mut done) = (None, None, None);
    let (mut cpu_start, mut cpu_done) = (None, None);
    let mut output = None;
    let mut closed = 0;
    let mut tail: Vec<String> = Vec::new();
    let outcome: Result<()> = loop {
        if cancel::is_cancelled() {
            break Err(Error::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Line::Text(line)) => {
                if tail.len() >= 20 {
                    tail.remove(0);
                }
                tail.push(line.clone());
                let at = started.elapsed();
                if line.contains("nailong-bench:ready") {
                    ready = Some(at);
                } else if line.contains("nailong-bench:start") {
                    start = Some(at);
                    cpu_start = cpu_secs(&child);
                } else if line.contains("nailong-bench:done") {
                    done = Some(at);
                    cpu_done = cpu_secs(&child);
                } else if let Some(parsed) = parse_result(&line) {
                    output = Some(parsed);
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
        if elapsed > budget || (ready.is_none() && elapsed > READY_TIMEOUT) {
            break Err(Error::Launch("测试程序没有按时跑完".into()));
        }
        let within = match (ready, start) {
            (None, _) => 0.2 * (elapsed.as_secs_f32() / 6.0).min(1.0),
            (Some(at), None) => {
                0.2 + 0.15 * ((elapsed - at).as_secs_f32() / tier.warmup_secs as f32).min(1.0)
            }
            (Some(_), Some(at)) => {
                0.35 + 0.65 * ((elapsed - at).as_secs_f32() / MEASURE_SECS as f32).min(1.0)
            }
        };
        let phase = match (ready, start) {
            (None, _) => "准备数据",
            (Some(_), None) => "预热",
            _ => "测量",
        };
        meter.report(format!("{stage} · {phase}"), within);
    };
    let _ = child.kill();
    let _ = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
    outcome?;
    let (Some(start), Some(done), Some(out)) = (start, done, output) else {
        return Err(Error::Launch("测试没有完整跑完".into()));
    };

    let events = std::fs::read_to_string(&gc_log)
        .map(|text| gclog::parse(&text))
        .unwrap_or_default();
    let measured = (done - start).as_secs_f64();
    let cpu_cores = match (cpu_start, cpu_done) {
        (Some(a), Some(b)) if measured > 0.0 => Some((b - a) / measured),
        _ => None,
    };
    // 日志的时间从 JVM 启动算，和我们看到标记的时间差一点启动开销，这里不细分。
    let jvm_start = start.as_secs_f64();
    Ok(RunReport {
        collector,
        heap_mb,
        fps: out.frames as f64 / out.secs,
        low_fps: 1000.0 / out.frame_p99.max(0.001),
        frame_max_ms: out.frame_max,
        hitches_per_min: out.hitches as f64 * 60.0 / out.secs,
        tick_p95_ms: out.tick_p95,
        tick_max_ms: out.tick_max,
        chunk_ratio: if out.chunk_target > 0.0 {
            out.chunks / out.chunk_target
        } else {
            1.0
        },
        cpu_cores,
        gc: gclog::summarize(&events, jvm_start - 1.0, jvm_start + measured + 1.0),
    })
}

/// 测试程序提前退出时，从最后几行输出里找出原因。
fn exit_reason(tail: &[String]) -> String {
    let hint = tail
        .iter()
        .rev()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("error") || lower.contains("exception") || lower.contains("could not")
        })
        .map(|line| line.trim().chars().take(160).collect::<String>());
    match hint {
        Some(hint) => format!("测试程序提前退出：{hint}"),
        None => "测试程序提前退出".into(),
    }
}

/// 比较同一档的 G1 和 ZGC，分别给出一般版本和官方默认 ZGC 版本的结论，并写下理由。
fn decide(
    g1: &RunReport,
    zgc: &RunReport,
    threads: usize,
    notes: &mut Vec<String>,
) -> (Collector, Collector) {
    for run in [g1, zgc] {
        notes.push(format!(
            "{}：平均 {:.0} 帧，1% 低 {:.0} 帧，每分钟顿 {:.0} 次，tick P95 {:.1} ms{}",
            run.collector.label(),
            run.fps,
            run.low_fps,
            run.hitches_per_min,
            run.tick_p95_ms,
            run.cpu_cores
                .map(|c| format!("，CPU {c:.1} 核"))
                .unwrap_or_default()
        ));
    }
    let extra_cores = match (zgc.cpu_cores, g1.cpu_cores) {
        (Some(z), Some(g)) => z - g,
        _ => 0.0,
    };
    let cpu_ok = |share: f64| extra_cores <= threads as f64 * share;
    // tick 的预算是 50 ms：P95 在半个 tick 以内就不会卡服务端，几毫秒的差别不算拖慢。
    let keeps_up = |share: f64| {
        zgc.fps >= g1.fps * share
            && zgc.chunk_ratio >= g1.chunk_ratio * share
            && zgc.tick_p95_ms <= (g1.tick_p95_ms * (2.0 - share) + 1.0).max(TICK_MS / 2.0)
    };
    // 一般版本：G1 已经不顿就不换。ZGC 要更顺，而且不拖慢、不多吃 CPU。
    // 十几秒里偶尔一次顿是噪声：至少每分钟 10 次（一轮里两次以上）才算。
    let g1_stutters = g1.hitches_per_min >= 10.0 || g1.frame_max_ms - 1000.0 / g1.fps >= 25.0;
    let zgc_smoother = (zgc.hitches_per_min <= g1.hitches_per_min / 2.0
        && zgc.low_fps >= g1.low_fps)
        || zgc.low_fps >= g1.low_fps * 1.1;
    let collector = if g1_stutters && zgc_smoother && keeps_up(0.97) && cpu_ok(0.1) {
        Collector::Zgc
    } else {
        Collector::G1
    };
    // 官方默认 ZGC：Mojang 在大量机器上验证过，除非它在这台电脑上明显更差，否则沿用。
    let zgc_worse = zgc.low_fps < g1.low_fps * 0.85
        || zgc.hitches_per_min > g1.hitches_per_min * 1.5 + 8.0
        || !keeps_up(0.95)
        || !cpu_ok(0.2);
    let official = if zgc_worse {
        Collector::G1
    } else {
        Collector::Zgc
    };
    if extra_cores > 0.05 {
        notes.push(format!("ZGC 平均多用 {extra_cores:.1} 个核"));
    }
    notes.push(format!(
        "结论：一般版本用 {}；官方默认 ZGC 的版本用 {}",
        collector.label(),
        official.label()
    ));
    (collector, official)
}

/// 在这台电脑的每个 Java 21+ 上跑完测试，保存并返回结果。
pub fn run(root: &Root, sink: &Sink) -> Result<Store> {
    cancel::check()?;
    cleanup_legacy(root);
    let machine = Machine::detect();
    let javas = java::find_all(root);
    let (picked, mut skipped) = candidates(&javas);
    if picked.is_empty() {
        return Err(Error::Launch(
            "这台电脑没有 64 位的 Java 21 或更新版本：只能用 G1，不需要测试".into(),
        ));
    }
    let dir = root.cache().join("jvmbench");
    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let class = dir.join(format!("{CLASS_NAME}.class"));
    if std::fs::read(&class).ok().as_deref() != Some(CLASS) {
        std::fs::write(&class, CLASS).map_err(|e| io(&class, e))?;
    }

    // 电脑放得下才跑大档：ZGC 那一轮的堆加上 JVM 自己的开销。
    let cap = machine
        .total_mb
        .map(|total| total.saturating_sub((total / 5).clamp(1536, 4096) + 768));
    let tiers: Vec<(&Tier, u32, u32)> = TIERS
        .iter()
        .filter_map(|tier| {
            let g1 = tuning::g1_heap_for(tier.live_mb());
            let zgc = tuning::zgc_heap_for(tier.live_mb());
            let fits = cap.is_none_or(|cap| zgc <= cap);
            if !fits {
                skipped.push(format!(
                    "{}档：电脑内存放不下 {} 的测试堆",
                    tier.label,
                    gb(zgc)
                ));
            }
            fits.then_some((tier, g1, zgc))
        })
        .collect();
    if tiers.is_empty() {
        return Err(Error::Launch("电脑内存太少，放不下测试用的堆".into()));
    }

    let mut meter = Meter {
        sink,
        round: 0,
        rounds: picked.len() * tiers.len() * 2,
    };
    let mut results = Vec::new();
    for java in &picked {
        let mut result = JavaResult {
            path: java.path.clone(),
            major: java.major,
            version: java.version.clone(),
            vendor: java.vendor.clone(),
            tiers: Vec::new(),
            notes: Vec::new(),
        };
        for (tier, g1_heap, zgc_heap) in &tiers {
            // 跑之前再看一次空闲内存：别的程序占着时，大档会被换到交换区，数字就不准了。
            let available = Machine::detect().available_mb;
            if tier.key != "standard" && available.is_some_and(|free| free < zgc_heap + 1024) {
                result.notes.push(format!(
                    "{}档：现在空闲内存不到 {}，先跳过",
                    tier.label,
                    gb(zgc_heap + 1024)
                ));
                meter.round += 2;
                continue;
            }
            let stage = |collector: Collector| {
                format!(
                    "Java {} · {}档 · {}",
                    java.major,
                    tier.label,
                    collector.label()
                )
            };
            let g1 = run_once(
                &dir,
                java,
                tier,
                Collector::G1,
                *g1_heap,
                &stage(Collector::G1),
                &meter,
            )?;
            meter.round += 1;
            let zgc = run_once(
                &dir,
                java,
                tier,
                Collector::Zgc,
                *zgc_heap,
                &stage(Collector::Zgc),
                &meter,
            )?;
            meter.round += 1;
            let mut notes = Vec::new();
            let (collector, official) = decide(&g1, &zgc, machine.threads, &mut notes);
            result.tiers.push(TierResult {
                key: tier.key.to_string(),
                label: tier.label.to_string(),
                live_mb: tier.live_mb(),
                g1,
                zgc,
                collector,
                official,
                notes,
            });
        }
        if result.tiers.iter().any(|tier| tier.key == "standard") {
            results.push(result);
        }
    }
    let store = Store {
        when: now(),
        threads: machine.threads,
        total_gb: total_gb(&machine),
        javas: results,
        skipped,
    };
    save(root, &store)?;
    let _ = std::fs::remove_file(dir.join(GC_LOG));
    (sink)(Progress {
        stage: "测试完成".into(),
        files_done: 1000,
        files_total: 1000,
        ..Default::default()
    });
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_result_line() {
        let line = "nailong-bench:result secs=12.000 frames=3138 frame_p50=3.768 frame_p99=4.420 \
                    frame_max=9.490 hitches=5 ticks=240 tick_p50=2.044 tick_p95=3.197 \
                    tick_max=7.734 chunks=1440 chunk_target=1440.016 sink=148";
        let out = parse_result(line).unwrap();
        assert_eq!(out.frames, 3138);
        assert_eq!(out.hitches, 5);
        assert!((out.frame_p99 - 4.42).abs() < 1e-9);
        assert!((out.chunk_target - 1440.016).abs() < 1e-9);
        assert_eq!(parse_result("nailong-bench:result secs=0 frames=0"), None);
        assert_eq!(parse_result("nailong-bench:done"), None);
    }

    #[test]
    fn the_embedded_program_is_java8_bytecode() {
        // 魔数 + 次版本 + 主版本 52（Java 8）：老电脑上只有 Java 8 也能加载。
        assert_eq!(&CLASS[..4], &[0xCA, 0xFE, 0xBA, 0xBE]);
        assert_eq!(u16::from_be_bytes([CLASS[6], CLASS[7]]), 52);
    }

    fn rt(major: u32, path: &str, arch: &str) -> JavaRuntime {
        JavaRuntime {
            path: PathBuf::from(path),
            major,
            version: major.to_string(),
            vendor: None,
            arch: Some(arch.into()),
        }
    }

    #[test]
    fn tests_one_openjdk_per_major_from_21() {
        let all = [
            rt(25, "/jvm/java-25/bin/java", "amd64"),
            rt(21, "/jvm/graalvm-21/bin/java", "amd64"),
            rt(21, "/jvm/java-21/bin/java", "amd64"),
            rt(23, "/jvm/graalvm-23/bin/java", "amd64"),
            rt(17, "/jvm/java-17/bin/java", "amd64"),
        ];
        let (picked, skipped) = candidates(&all);
        let paths: Vec<_> = picked.iter().map(|j| j.path.clone()).collect();
        assert_eq!(
            paths,
            [
                PathBuf::from("/jvm/java-21/bin/java"),
                PathBuf::from("/jvm/java-25/bin/java")
            ]
        );
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("GraalVM"));
    }

    fn report(collector: Collector, fps: f64, low: f64, hitches: f64, cpu: f64) -> RunReport {
        RunReport {
            collector,
            heap_mb: 2048,
            fps,
            low_fps: low,
            frame_max_ms: 1000.0 / low + 2.0,
            hitches_per_min: hitches,
            tick_p95_ms: 3.0,
            tick_max_ms: 8.0,
            chunk_ratio: 1.0,
            cpu_cores: Some(cpu),
            gc: GcSummary::default(),
        }
    }

    #[test]
    fn zgc_has_to_earn_its_place() {
        let mut notes = Vec::new();
        let smooth_g1 = report(Collector::G1, 260.0, 226.0, 0.0, 3.0);
        let stutter_g1 = report(Collector::G1, 260.0, 150.0, 12.0, 3.0);
        let zgc = report(Collector::Zgc, 270.0, 240.0, 0.0, 3.5);
        let slow_zgc = report(Collector::Zgc, 230.0, 200.0, 0.0, 3.5);
        let hungry_zgc = report(Collector::Zgc, 262.0, 240.0, 0.0, 6.0);
        // G1 已经很顺：一般版本留 G1，官方默认 ZGC 的版本沿用 ZGC。
        assert_eq!(
            decide(&smooth_g1, &zgc, 16, &mut notes),
            (Collector::G1, Collector::Zgc)
        );
        // G1 常顿、ZGC 更顺且不拖慢：换 ZGC。
        assert_eq!(
            decide(&stutter_g1, &zgc, 16, &mut notes),
            (Collector::Zgc, Collector::Zgc)
        );
        // ZGC 拖慢了平均帧率：哪种版本都不用它。
        assert_eq!(
            decide(&smooth_g1, &slow_zgc, 16, &mut notes),
            (Collector::G1, Collector::G1)
        );
        // 一轮里只顿了一下（每分钟 5 次）：两边都算噪声，不推翻任何默认。
        let one_hitch_g1 = report(Collector::G1, 260.0, 226.0, 5.0, 3.0);
        let one_hitch_zgc = report(Collector::Zgc, 262.0, 205.0, 5.0, 3.0);
        assert_eq!(
            decide(&one_hitch_g1, &zgc, 16, &mut notes),
            (Collector::G1, Collector::Zgc)
        );
        assert_eq!(
            decide(&smooth_g1, &one_hitch_zgc, 16, &mut notes),
            (Collector::G1, Collector::Zgc)
        );
        // ZGC 多吃 3 个核：超过 16 线程的 10%，一般版本留 G1；没超过 20%，官方版本仍用 ZGC。
        assert_eq!(
            decide(&stutter_g1, &hungry_zgc, 16, &mut notes),
            (Collector::G1, Collector::Zgc)
        );
    }

    /// 2026-11-04 在 i5-13490F 16 线程、16 GB 上的第二次实测。
    #[test]
    fn real_run_on_a_16_thread_machine() {
        let run = |collector, fps, low, hitches, tick, cpu| RunReport {
            tick_p95_ms: tick,
            ..report(collector, fps, low, hitches, cpu)
        };
        let mut notes = Vec::new();
        let cases = [
            // Java 21 常规：两边都不顿，一般版本留 G1，官方默认沿用 ZGC。
            (
                run(Collector::G1, 264.0, 237.0, 0.0, 3.6, 1.2),
                run(Collector::Zgc, 275.0, 240.0, 0.0, 3.5, 1.1),
                (Collector::G1, Collector::Zgc),
            ),
            // Java 21 大型整合包：G1 每分钟顿 25 次，ZGC 不顿；tick 从 3.8 到 5.0 ms 不算拖慢。
            (
                run(Collector::G1, 256.0, 222.0, 25.0, 3.8, 1.5),
                run(Collector::Zgc, 270.0, 226.0, 0.0, 5.0, 1.3),
                (Collector::Zgc, Collector::Zgc),
            ),
            // Java 25 常规：G1 一轮里只顿了一下，算噪声。
            (
                run(Collector::G1, 331.0, 287.0, 5.0, 2.5, 1.3),
                run(Collector::Zgc, 355.0, 289.0, 0.0, 3.7, 1.1),
                (Collector::G1, Collector::Zgc),
            ),
            (
                run(Collector::G1, 327.0, 257.0, 20.0, 3.8, 1.5),
                run(Collector::Zgc, 354.0, 274.0, 0.0, 5.2, 1.3),
                (Collector::Zgc, Collector::Zgc),
            ),
        ];
        for (g1, zgc, expected) in cases {
            assert_eq!(decide(&g1, &zgc, 16, &mut notes), expected);
        }
        // tick 真的逼近预算时仍然算拖慢。
        let slow_ticks = run(Collector::Zgc, 270.0, 226.0, 0.0, 30.0, 1.3);
        let g1 = run(Collector::G1, 256.0, 222.0, 25.0, 3.8, 1.5);
        assert_eq!(
            decide(&g1, &slow_ticks, 16, &mut notes),
            (Collector::G1, Collector::G1)
        );
    }

    #[test]
    fn verdict_picks_by_path_then_major() {
        let tier = |key: &str, collector| TierResult {
            key: key.into(),
            label: String::new(),
            live_mb: 1000,
            g1: report(Collector::G1, 1.0, 1.0, 0.0, 1.0),
            zgc: report(Collector::Zgc, 1.0, 1.0, 0.0, 1.0),
            collector,
            official: Collector::Zgc,
            notes: Vec::new(),
        };
        let machine = Machine::detect();
        let store = Store {
            when: now(),
            threads: machine.threads,
            total_gb: total_gb(&machine),
            javas: vec![JavaResult {
                path: PathBuf::from("/jvm/java-21/bin/java"),
                major: 21,
                version: "21".into(),
                vendor: None,
                tiers: vec![
                    tier("standard", Collector::G1),
                    tier("large", Collector::Zgc),
                ],
                notes: Vec::new(),
            }],
            skipped: Vec::new(),
        };
        let other = rt(21, "/opt/temurin-21/bin/java", "amd64");
        let verdict = store.verdict(&other).unwrap();
        assert_eq!(verdict.standard.collector, Collector::G1);
        assert_eq!(verdict.large.unwrap().collector, Collector::Zgc);
        assert!(
            store
                .verdict(&rt(25, "/jvm/java-25/bin/java", "amd64"))
                .is_none()
        );
        assert!(
            store
                .verdict(&rt(21, "/jvm/graalvm-21/bin/java", "amd64"))
                .is_none()
        );
    }
}
