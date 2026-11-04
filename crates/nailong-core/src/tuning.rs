//! 按电脑配置、Java 版本和 Mod 数量算出游戏的堆大小和 GC 参数。
//!
//! 取值和依据见 `docs/design/19-jvm-tuning.md`。这里只放已在对应 Java 版本上
//! 验证过的参数：JVM 遇到不认识的参数会直接拒绝启动。

use crate::error::Result;
use crate::java::{self, JavaRuntime};
use crate::manifest::{self, Context, Resolved};
use crate::paths::Root;
use std::collections::HashSet;
use std::path::Path;

/// 自动模式最多给这么大的堆。再大只会让每次回收停得更久。
const AUTO_MAX_MB: u32 = 12 * 1024;
const AUTO_MIN_MB: u32 = 2048;
const MIN_HEAP_MB: u32 = 1024;
/// 32 位 Java 的地址空间放不下更大的堆。
const MAX_32BIT_MB: u32 = 1024;
/// 读不到电脑内存时沿用以前的默认值。
const UNKNOWN_MACHINE_MB: u32 = 4096;
/// 实验参数只在验证过的 Java 版本上使用；更新的版本只用正式参数。
const NEWEST_VERIFIED_JAVA: u32 = 25;

/// 自动选 ZGC 的门槛：分代 ZGC、足够的核和内存，而且堆大到 G1 的混合回收开始明显。
const ZGC_MIN_JAVA: u32 = 21;
const ZGC_MIN_THREADS: usize = 12;
const ZGC_MIN_TOTAL_MB: u32 = 24 * 1024;
const ZGC_MIN_HEAP_MB: u32 = 8 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub struct Machine {
    /// 物理内存，MB；读不到时为 None。
    pub total_mb: Option<u32>,
    /// 此刻可用的内存，MB。
    pub available_mb: Option<u32>,
    /// 逻辑处理器数，含超线程。
    pub threads: usize,
}

impl Machine {
    pub fn detect() -> Self {
        let (total_mb, available_mb) = memory();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            total_mb,
            available_mb,
            threads,
        }
    }
}

#[cfg(target_os = "linux")]
fn memory() -> (Option<u32>, Option<u32>) {
    match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => parse_meminfo(&text),
        Err(_) => (None, None),
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> (Option<u32>, Option<u32>) {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map(|kb| (kb / 1024) as u32)
    };
    (field("MemTotal:"), field("MemAvailable:"))
}

#[cfg(windows)]
fn memory() -> (Option<u32>, Option<u32>) {
    #[repr(C)]
    #[allow(dead_code)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: 布局与 MEMORYSTATUSEX 一致，dwLength 已按要求填好。
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return (None, None);
    }
    let mb = |bytes: u64| Some((bytes / 1024 / 1024) as u32);
    (mb(status.total_phys), mb(status.avail_phys))
}

#[cfg(target_os = "macos")]
fn memory() -> (Option<u32>, Option<u32>) {
    let total = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|bytes| (bytes / 1024 / 1024) as u32);
    (total, None)
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn memory() -> (Option<u32>, Option<u32>) {
    (None, None)
}

/// 设置里的回收器选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GcChoice {
    #[default]
    Auto,
    G1,
    Zgc,
}

impl GcChoice {
    pub const ALL: [GcChoice; 3] = [GcChoice::Auto, GcChoice::G1, GcChoice::Zgc];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::G1 => "G1",
            Self::Zgc => "ZGC",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::G1 => "g1",
            Self::Zgc => "zgc",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key.to_ascii_lowercase().as_str() {
            "g1" => Self::G1,
            "zgc" => Self::Zgc,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    Vanilla,
    Fabric,
    Forge,
}

impl Loader {
    pub fn from_main_class(main_class: &str) -> Self {
        let main = main_class.to_ascii_lowercase();
        if main.contains("fabricmc") || main.contains("quiltmc") {
            Self::Fabric
        } else if [
            "forge",
            "neoforged",
            "modlauncher",
            "bootstraplauncher",
            "launchwrapper",
        ]
        .iter()
        .any(|key| main.contains(key))
        {
            Self::Forge
        } else {
            Self::Vanilla
        }
    }
}

/// 这次启动要跑的东西。
#[derive(Debug, Clone)]
pub struct Workload {
    pub java_major: u32,
    pub java_64bit: bool,
    pub graal: bool,
    pub loader: Loader,
    /// `mods` 目录里启用的 jar 数量；原版为 0。
    pub mods: usize,
    /// 版本文件 `default-user-jvm` 里 Mojang 给官方启动器的默认参数（26.1 起才有），
    /// 已按当前系统筛过规则。
    pub official: Vec<String>,
}

impl Workload {
    pub fn new(java: &JavaRuntime, resolved: &Resolved, game_dir: &Path) -> Self {
        let loader = Loader::from_main_class(&resolved.main_class);
        let mods = if loader == Loader::Vanilla {
            0
        } else {
            count_mods(&game_dir.join("mods"))
        };
        Self {
            java_major: java.major,
            java_64bit: java.is_64bit(),
            graal: java.is_graal(),
            loader,
            mods,
            official: crate::launch::expand_args(
                &resolved.arguments.default_user_jvm,
                &Context::current(),
            ),
        }
    }

    fn official_has(&self, flag: &str) -> bool {
        self.official.iter().any(|arg| arg == flag)
    }

    /// 官方默认里的初始堆，例如 `-Xms2G` 得到 2048。
    fn official_xms(&self) -> Option<u32> {
        self.official
            .iter()
            .find_map(|arg| arg.strip_prefix("-Xms"))
            .and_then(parse_size_mb)
    }
}

/// `2G`、`512m`、`1024` 这类 JVM 内存写法换算成 MB。
fn parse_size_mb(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let (digits, scale) = match lower.chars().last()? {
        'g' => (&lower[..lower.len() - 1], 1024),
        'm' => (&lower[..lower.len() - 1], 1),
        'k' => {
            return lower[..lower.len() - 1]
                .parse::<u32>()
                .ok()
                .map(|k| k / 1024);
        }
        _ => return lower.parse::<u64>().ok().map(|b| (b / 1024 / 1024) as u32),
    };
    digits.parse::<u32>().ok().map(|n| n * scale)
}

/// 按启动时会选中的 Java 读出负载，界面用它预览参数。
pub fn workload(
    root: &Root,
    profile: &str,
    game_dir: &Path,
    javas: &[JavaRuntime],
) -> Result<Workload> {
    let resolved = manifest::resolve(root, profile, &Context::current())?;
    let java = java::select(crate::launch::required_java(&resolved), javas)?;
    Ok(Workload::new(&java, &resolved, game_dir))
}

fn count_mods(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".jar")
        })
        .count()
}

/// 用户在设置里能改的部分。
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// 手动指定的最大堆，MB；None 表示自动。
    pub memory_mb: Option<u32>,
    /// 手动指定的初始堆，MB；None 时按内存是否宽裕决定。
    pub min_memory_mb: Option<u32>,
    pub gc: GcChoice,
    /// 用户自己写的 JVM 参数。排在最后，和自动参数冲突时以它为准。
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collector {
    G1,
    Zgc,
}

impl Collector {
    pub fn label(self) -> &'static str {
        match self {
            Self::G1 => "G1",
            Self::Zgc => "ZGC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// 堆大小，用户写了 -Xmx 或 -Xms 时整组让给用户。
    Heap,
    /// 只属于所选回收器，用户指定了别的回收器时整组去掉。
    Gc,
    /// 解锁实验参数。必须排在实验参数前面，所以从不去重。
    Unlock,
    Other,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub heap_mb: u32,
    pub min_heap_mb: u32,
    pub collector: Collector,
    pub auto_memory: bool,
    /// 例如「按 16 GB 内存、16 线程、Java 21、87 个 Mod 计算」。
    pub basis: String,
    /// 计算过程，一行一条，给设置页和 `--dry-run` 看。
    pub steps: Vec<String>,
    /// 需要用户留意的事。
    pub warnings: Vec<String>,
    flags: Vec<(String, Kind)>,
    extra: Vec<String>,
}

impl Plan {
    /// 例如「G1 · 5.5 GB」。
    pub fn headline(&self) -> String {
        format!("{} · {}", self.collector.label(), gb(self.heap_mb))
    }

    /// 最终交给 JVM 的参数（不含版本文件自带的）。
    pub fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        self.apply(&mut args);
        args
    }

    /// 把参数接到 `jvm` 后面。`jvm` 里已有的和用户写的参数优先。
    pub fn apply(&self, jvm: &mut Vec<String>) {
        let given: Vec<&str> = jvm
            .iter()
            .chain(self.extra.iter())
            .map(String::as_str)
            .collect();
        let custom_gc = given.iter().any(|arg| selects_gc(arg));
        let taken: HashSet<&str> = given.iter().filter_map(|arg| flag_name(arg)).collect();
        let custom_heap = taken.contains("Xmx") || taken.contains("Xms");
        let mut out = Vec::new();
        for (flag, kind) in &self.flags {
            let keep = match kind {
                Kind::Unlock => true,
                Kind::Heap => !custom_heap,
                Kind::Gc => !custom_gc && flag_name(flag).is_none_or(|name| !taken.contains(name)),
                Kind::Other => flag_name(flag).is_none_or(|name| !taken.contains(name)),
            };
            if keep {
                out.push(flag.clone());
            }
        }
        jvm.extend(out);
        jvm.extend(self.extra.iter().cloned());
    }
}

/// `-XX:+Foo`、`-XX:Foo=1` 得到 `Foo`；`-Xmx4G` 得到 `Xmx`。
fn flag_name(arg: &str) -> Option<&str> {
    if let Some(rest) = arg.strip_prefix("-XX:") {
        let rest = rest.strip_prefix(['+', '-']).unwrap_or(rest);
        return rest.split('=').next();
    }
    ["-Xmx", "-Xms", "-Xmn", "-Xss"]
        .into_iter()
        .find(|prefix| arg.starts_with(prefix))
        .map(|prefix| &prefix[1..])
}

fn selects_gc(arg: &str) -> bool {
    arg.starts_with("-XX:+Use") && arg.ends_with("GC")
}

/// 按空白拆分用户填的参数，支持单双引号。
pub fn split_args(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => current.push(c),
        }
    }
    if started || !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn gb(mb: u32) -> String {
    if mb.is_multiple_of(1024) {
        format!("{} GB", mb / 1024)
    } else {
        format!("{:.1} GB", mb as f32 / 1024.0)
    }
}

fn ceil_to(mb: u32, step: u32) -> u32 {
    mb.div_ceil(step) * step
}

fn floor_to(mb: u32, step: u32) -> u32 {
    mb / step * step
}

/// 游戏本身大约要多少堆：基础 + 加载器 + 每个 Mod。
fn demand(work: &Workload) -> (u32, String) {
    // 1.18 起世界变高、区块更大，同时要求 Java 17。
    let base = if work.java_major >= 17 { 3072 } else { 2048 };
    let (loader, per_mod) = match work.loader {
        Loader::Vanilla => (0, 0),
        Loader::Fabric => (256, 10),
        Loader::Forge => (512, 20),
    };
    let mods = work.mods.min(10_000) as u32;
    let raw = base + loader + per_mod * mods;
    let need = ceil_to(raw, 512).clamp(AUTO_MIN_MB, AUTO_MAX_MB);
    let detail = if work.loader == Loader::Vanilla {
        format!("需要 {}：原版基础 {}", gb(need), gb(base))
    } else {
        format!(
            "需要 {}：基础 {} + 加载器 {} MB + {} 个 Mod × {} MB",
            gb(need),
            gb(base),
            loader,
            mods,
            per_mod
        )
    };
    (need, detail)
}

/// 堆之外 JVM 自己要用的：类元数据、编译后的代码、线程栈和本地内存。
fn non_heap(work: &Workload) -> u32 {
    (768 + 2 * work.mods.min(10_000) as u32).min(2048)
}

/// 留给系统和其他程序的内存。
fn reserve(total: u32) -> u32 {
    (total / 5).clamp(1536, 4096)
}

fn region_mb(heap: u32) -> u32 {
    match heap {
        0..=4096 => 8,
        4097..=8192 => 16,
        _ => 32,
    }
}

pub fn plan(machine: &Machine, work: &Workload, settings: &Settings) -> Plan {
    let mut steps = Vec::new();
    let mut warnings = Vec::new();

    let (need, detail) = demand(work);
    steps.push(detail);

    let nonheap = non_heap(work);
    let mut cap = machine.total_mb.map(|total| {
        let reserve = reserve(total);
        let cap = floor_to(total.saturating_sub(reserve + nonheap), 512).max(MIN_HEAP_MB);
        steps.push(format!(
            "上限 {}：{} 内存 − 系统预留 {} − Java 自身 {}",
            gb(cap),
            gb(total),
            gb(reserve),
            gb(nonheap)
        ));
        cap
    });
    if machine.total_mb.is_none() {
        steps.push(format!(
            "没读到电脑内存，按 {} 处理",
            gb(UNKNOWN_MACHINE_MB)
        ));
    }
    if !work.java_64bit {
        cap = Some(cap.unwrap_or(MAX_32BIT_MB).min(MAX_32BIT_MB));
        steps.push(format!("32 位 Java 最多 {}", gb(MAX_32BIT_MB)));
    }

    // ZGC 回收时不暂停，但要多留空间给并发回收，所以多给 1/4。
    let zgc_heap = ceil_to(need + need / 4, 512);
    let zgc_ok = work.java_major >= 17 && work.java_64bit;
    let collector = match settings.gc {
        GcChoice::G1 => {
            steps.push("回收器 G1：手动选择".into());
            Collector::G1
        }
        GcChoice::Zgc if zgc_ok => {
            if work.java_major < ZGC_MIN_JAVA {
                warnings.push(format!(
                    "Java {} 只有不分代的 ZGC，帧率可能比 G1 低",
                    work.java_major
                ));
            }
            if work.graal {
                warnings.push("GraalVM 用 ZGC 时会关掉 Graal 编译器".into());
            }
            steps.push("回收器 ZGC：手动选择".into());
            Collector::Zgc
        }
        GcChoice::Zgc => {
            warnings.push(if work.java_64bit {
                format!("Java {} 不支持 ZGC，改用 G1", work.java_major)
            } else {
                "32 位 Java 不支持 ZGC，改用 G1".into()
            });
            Collector::G1
        }
        GcChoice::Auto if !work.official.is_empty() => {
            // 26.1 起 Mojang 在版本文件里写了官方启动器的默认回收器：
            // 新系统用 ZGC，Windows 10 1803 以前用 G1。这是他们在大量机器上验证过的选择。
            if work.official_has("-XX:+UseZGC") && zgc_ok {
                steps.push("回收器 ZGC：这个版本的官方默认".into());
                Collector::Zgc
            } else {
                steps.push("回收器 G1：这个版本在本系统上的官方默认".into());
                Collector::G1
            }
        }
        GcChoice::Auto => {
            let strong = work.java_major >= ZGC_MIN_JAVA
                && zgc_ok
                && !work.graal
                && machine.threads >= ZGC_MIN_THREADS
                && machine
                    .total_mb
                    .is_some_and(|total| total >= ZGC_MIN_TOTAL_MB)
                && need >= ZGC_MIN_HEAP_MB
                && cap.is_some_and(|cap| cap >= zgc_heap);
            if strong {
                steps.push("回收器 ZGC：堆很大，核和内存也够，用分代 ZGC 消除回收停顿".into());
                Collector::Zgc
            } else {
                steps.push(format!(
                    "回收器 G1：ZGC 要 Java {ZGC_MIN_JAVA}+、{ZGC_MIN_THREADS} 线程、{} 内存、堆 {} 以上才划算",
                    gb(ZGC_MIN_TOTAL_MB),
                    gb(ZGC_MIN_HEAP_MB)
                ));
                Collector::G1
            }
        }
    };

    let auto_memory = settings.memory_mb.is_none();
    let heap = match settings.memory_mb {
        Some(manual) => {
            let mut heap = manual.max(MIN_HEAP_MB);
            if !work.java_64bit && heap > MAX_32BIT_MB {
                warnings.push(format!(
                    "32 位 Java 最多 {}，已按这个值启动",
                    gb(MAX_32BIT_MB)
                ));
                heap = MAX_32BIT_MB;
            } else if let Some(cap) = cap
                && heap > cap
            {
                warnings.push(format!(
                    "手动设置的 {} 超过建议上限 {}，可能挤占系统内存",
                    gb(heap),
                    gb(cap)
                ));
            }
            steps.push(format!("最大堆 {}：手动设置", gb(heap)));
            heap
        }
        None => {
            let want = if collector == Collector::Zgc {
                zgc_heap
            } else {
                need
            };
            let limit = cap.unwrap_or(UNKNOWN_MACHINE_MB);
            let heap = want.min(limit).max(MIN_HEAP_MB);
            if heap < want {
                steps.push(format!("最大堆 {}：受电脑内存限制", gb(heap)));
                if heap < need {
                    warnings.push(format!(
                        "电脑内存偏少，这个版本建议 {}，现在只能给 {}",
                        gb(need),
                        gb(heap)
                    ));
                }
            } else {
                steps.push(format!("最大堆 {}", gb(heap)));
            }
            heap
        }
    };

    let min_heap = match settings.min_memory_mb {
        Some(min) => min.clamp(MIN_HEAP_MB.min(heap), heap),
        // 官方默认用 AlwaysPreTouch 在启动时把初始堆写一遍，初始堆太大会拖慢启动，
        // 所以沿用官方的初始堆。
        None if work.official_has("-XX:+AlwaysPreTouch") => {
            let xms = work.official_xms().unwrap_or(2048).min(heap);
            steps.push(format!("初始堆 {}：沿用官方默认，启动时预先占好", gb(xms)));
            xms
        }
        None => {
            // 初始堆在启动时就要占住；ZGC 还会立刻向系统要实打实的内存，
            // 不够就直接启动失败。所以看的是此刻空闲的内存，而不是总内存。
            let roomy = match (machine.available_mb, machine.total_mb) {
                (Some(available), _) => available >= heap + nonheap + 1024,
                (None, Some(total)) => total >= heap + nonheap + reserve(total) + 2048,
                (None, None) => false,
            };
            if roomy {
                steps.push("初始堆 = 最大堆：内存宽裕，运行中不用扩容".into());
                heap
            } else {
                steps.push("初始堆取一半：现在空闲内存不多，用多少占多少".into());
                floor_to(heap / 2, 256).clamp(MIN_HEAP_MB.min(heap), heap)
            }
        }
    };

    if let Some(available) = machine.available_mb
        && available < heap + nonheap
    {
        warnings.push(format!(
            "现在只剩 {} 可用内存，先关掉一些程序会更流畅",
            gb(floor_to(available, 128))
        ));
    }

    let verified = work.java_major <= NEWEST_VERIFIED_JAVA;
    let mut flags = vec![
        (format!("-Xms{min_heap}m"), Kind::Heap),
        (format!("-Xmx{heap}m"), Kind::Heap),
    ];
    match collector {
        Collector::G1 => {
            let region = region_mb(heap);
            flags.push(("-XX:+UseG1GC".into(), Kind::Gc));
            if verified {
                flags.push(("-XX:+UnlockExperimentalVMOptions".into(), Kind::Unlock));
                flags.push(("-XX:G1NewSizePercent=20".into(), Kind::Gc));
            }
            flags.push(("-XX:G1ReservePercent=20".into(), Kind::Gc));
            flags.push(("-XX:MaxGCPauseMillis=50".into(), Kind::Gc));
            flags.push((format!("-XX:G1HeapRegionSize={region}M"), Kind::Gc));
            // Java 11 起 G1 默认就是并行处理引用。
            if work.java_major < 11 {
                flags.push(("-XX:+ParallelRefProcEnabled".into(), Kind::Gc));
            }
            flags.push(("-XX:+ExplicitGCInvokesConcurrent".into(), Kind::Gc));
            if (25..=NEWEST_VERIFIED_JAVA).contains(&work.java_major) {
                flags.push(("-XX:+UseCompactObjectHeaders".into(), Kind::Gc));
            }
            steps.push(format!(
                "G1：新生代 20%、预留 20%、目标停顿 50 ms、Region {region} MB{}",
                if (25..=NEWEST_VERIFIED_JAVA).contains(&work.java_major) {
                    "，开启紧凑对象头"
                } else {
                    ""
                }
            ));
        }
        Collector::Zgc => {
            flags.push(("-XX:+UseZGC".into(), Kind::Gc));
            // Java 21、22 默认还是不分代；23 起分代是默认，24 起这个参数被废弃。
            if (21..=22).contains(&work.java_major) {
                flags.push(("-XX:+ZGenerational".into(), Kind::Gc));
            }
        }
    }
    // 官方默认里与回收器无关的参数（紧凑对象头、预先占用、字符串去重）照用；
    // 堆大小和回收器由上面算出来。
    for arg in &work.official {
        let name = flag_name(arg);
        let skip = arg.starts_with("-Xm")
            || selects_gc(arg)
            || name.is_some_and(|name| {
                name.starts_with("G1")
                    || name.starts_with('Z')
                    || name == "MaxGCPauseMillis"
                    || name.starts_with("Unlock")
                    || flags.iter().any(|(flag, _)| flag_name(flag) == Some(name))
            });
        if !skip {
            flags.push((arg.clone(), Kind::Other));
        }
    }
    flags.push(("-XX:+PerfDisableSharedMem".into(), Kind::Other));
    if !verified {
        steps.push(format!(
            "Java {} 比已验证的版本新，只用正式参数",
            work.java_major
        ));
    }

    Plan {
        heap_mb: heap,
        min_heap_mb: min_heap,
        collector,
        auto_memory,
        basis: basis(machine, work),
        steps,
        warnings,
        flags,
        extra: settings.extra.clone(),
    }
}

fn basis(machine: &Machine, work: &Workload) -> String {
    let mut parts = Vec::new();
    // 系统报告的总内存比标称少一点（显存、固件保留），向上取整到整 GB 更好认。
    if let Some(total) = machine.total_mb {
        parts.push(format!("{} GB 内存", total.div_ceil(1024)));
    }
    parts.push(format!("{} 线程", machine.threads));
    parts.push(format!("Java {}", work.java_major));
    if work.loader != Loader::Vanilla {
        parts.push(format!("{} 个 Mod", work.mods));
    }
    format!("按 {} 计算", parts.join("、"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(total_gb: u32, threads: usize) -> Machine {
        Machine {
            total_mb: Some(total_gb * 1024),
            // 开着浏览器之类的程序，平时大约一半空闲。
            available_mb: Some(total_gb * 512),
            threads,
        }
    }

    fn work(java_major: u32, loader: Loader, mods: usize) -> Workload {
        Workload {
            java_major,
            java_64bit: true,
            graal: false,
            loader,
            mods,
            official: Vec::new(),
        }
    }

    /// 26.3 版本文件在 Linux 上展开后的 `default-user-jvm`。
    fn official_26() -> Vec<String> {
        [
            "-Xms2G",
            "-Xmx4G",
            "-XX:+UseCompactObjectHeaders",
            "-XX:+AlwaysPreTouch",
            "-XX:+UseStringDeduplication",
            "-XX:+UseZGC",
        ]
        .map(String::from)
        .to_vec()
    }

    fn auto(machine: &Machine, work: &Workload) -> Plan {
        plan(machine, work, &Settings::default())
    }

    #[test]
    fn reads_meminfo() {
        let text = "MemTotal:       16303268 kB\nMemFree:  1 kB\nMemAvailable:    6710886 kB\n";
        assert_eq!(parse_meminfo(text), (Some(15921), Some(6553)));
    }

    #[test]
    fn vanilla_gets_three_gigabytes_on_a_normal_pc() {
        let plan = auto(&machine(16, 8), &work(21, Loader::Vanilla, 40));
        assert_eq!(plan.heap_mb, 3072);
        assert_eq!(plan.min_heap_mb, 3072);
        assert_eq!(plan.collector, Collector::G1);
        assert!(plan.args().contains(&"-XX:G1HeapRegionSize=8M".to_string()));
        assert_eq!(plan.basis, "按 16 GB 内存、8 线程、Java 21 计算");
    }

    #[test]
    fn old_versions_need_less() {
        let plan = auto(&machine(16, 8), &work(8, Loader::Vanilla, 0));
        assert_eq!(plan.heap_mb, 2048);
        let args = plan.args();
        assert!(args.contains(&"-XX:+ParallelRefProcEnabled".to_string()));
        assert!(!args.iter().any(|a| a.contains("CompactObjectHeaders")));
    }

    #[test]
    fn modpacks_scale_with_mod_count() {
        let pc = machine(16, 8);
        let fabric = auto(&pc, &work(21, Loader::Fabric, 70)).heap_mb;
        let forge = auto(&pc, &work(21, Loader::Forge, 150)).heap_mb;
        assert_eq!(fabric, 4096);
        assert_eq!(forge, 6656);
    }

    #[test]
    fn big_packs_stop_at_the_machine_limit() {
        let plan = auto(&machine(16, 8), &work(17, Loader::Forge, 430));
        // 16 GB − 3.2 GB 预留 − 1.6 GB 非堆 = 11.2 GB，向下取整到 11 GB。
        assert_eq!(plan.heap_mb, 11264);
        assert!(plan.min_heap_mb < plan.heap_mb);
        assert!(
            plan.args()
                .contains(&"-XX:G1HeapRegionSize=32M".to_string())
        );
        let plan = auto(&machine(32, 8), &work(17, Loader::Forge, 430));
        assert_eq!(plan.heap_mb, AUTO_MAX_MB);
    }

    #[test]
    fn small_machines_keep_room_for_the_system() {
        let plan = auto(&machine(8, 4), &work(21, Loader::Forge, 87));
        assert_eq!(plan.heap_mb, 5120);
        assert_eq!(plan.min_heap_mb, 2560);
        let plan = auto(&machine(4, 4), &work(21, Loader::Vanilla, 0));
        assert_eq!(plan.heap_mb, 1536);
        assert!(!plan.warnings.is_empty());
    }

    #[test]
    fn follows_the_official_defaults_when_the_version_has_them() {
        let mut w = work(25, Loader::Vanilla, 0);
        w.official = official_26();
        let plan = auto(&machine(16, 8), &w);
        assert_eq!(plan.collector, Collector::Zgc);
        // 原版需要 3 GB，ZGC 多给 1/4，正好是官方的 4 GB。
        assert_eq!(plan.heap_mb, 4096);
        assert_eq!(plan.min_heap_mb, 2048);
        let args = plan.args();
        for flag in [
            "-XX:+UseZGC",
            "-XX:+UseCompactObjectHeaders",
            "-XX:+AlwaysPreTouch",
            "-XX:+UseStringDeduplication",
        ] {
            assert_eq!(args.iter().filter(|a| *a == flag).count(), 1, "{flag}");
        }
        assert!(!args.iter().any(|a| a.contains("ZGenerational")));

        // 手动选 G1 时用我们的 G1 参数，官方的通用参数照留，紧凑对象头不重复。
        let g1 = plan_for_gc(&machine(16, 8), &w, GcChoice::G1).args();
        assert!(g1.contains(&"-XX:+UseG1GC".to_string()));
        assert!(!g1.contains(&"-XX:+UseZGC".to_string()));
        assert_eq!(
            g1.iter()
                .filter(|a| a.contains("CompactObjectHeaders"))
                .count(),
            1
        );

        // 旧版 Windows 上官方默认是 G1。
        w.official.retain(|a| a != "-XX:+UseZGC");
        w.official.push("-XX:+UseG1GC".into());
        assert_eq!(auto(&machine(16, 8), &w).collector, Collector::G1);
    }

    #[test]
    fn parses_jvm_sizes() {
        assert_eq!(parse_size_mb("2G"), Some(2048));
        assert_eq!(parse_size_mb("512m"), Some(512));
        assert_eq!(parse_size_mb("1048576k"), Some(1024));
        assert_eq!(parse_size_mb("x"), None);
    }

    #[test]
    fn initial_heap_follows_free_memory() {
        let mut pc = machine(32, 16);
        let huge = work(21, Loader::Forge, 300);
        assert_eq!(auto(&pc, &huge).min_heap_mb, AUTO_MAX_MB);
        // 总内存够，但此刻只剩 6 GB：ZGC 要是一开始就占 12 GB 会直接启动失败。
        pc.available_mb = Some(6 * 1024);
        let plan = auto(&pc, &huge);
        assert_eq!(plan.collector, Collector::Zgc);
        assert_eq!(plan.min_heap_mb, AUTO_MAX_MB / 2);
    }

    #[test]
    fn thirty_two_bit_java_is_capped() {
        let mut w = work(8, Loader::Forge, 100);
        w.java_64bit = false;
        let capped = auto(&machine(16, 8), &w);
        assert_eq!(capped.heap_mb, MAX_32BIT_MB);
        let manual = plan(
            &machine(16, 8),
            &w,
            &Settings {
                memory_mb: Some(4096),
                gc: GcChoice::Zgc,
                ..Default::default()
            },
        );
        assert_eq!(manual.heap_mb, MAX_32BIT_MB);
        assert_eq!(manual.collector, Collector::G1);
    }

    #[test]
    fn zgc_only_when_everything_is_strong() {
        let huge = work(21, Loader::Forge, 300);
        let plan = auto(&machine(32, 16), &huge);
        assert_eq!(plan.collector, Collector::Zgc);
        assert!(plan.args().contains(&"-XX:+ZGenerational".to_string()));
        assert_eq!(plan.heap_mb, AUTO_MAX_MB);
        assert_eq!(auto(&machine(32, 8), &huge).collector, Collector::G1);
        assert_eq!(auto(&machine(16, 16), &huge).collector, Collector::G1);
        assert_eq!(
            auto(&machine(32, 16), &work(21, Loader::Fabric, 80)).collector,
            Collector::G1
        );
        assert_eq!(
            auto(&machine(32, 16), &work(17, Loader::Forge, 300)).collector,
            Collector::G1
        );
        let mut graal = huge.clone();
        graal.graal = true;
        assert_eq!(auto(&machine(32, 16), &graal).collector, Collector::G1);
        let java25 = auto(&machine(32, 16), &work(25, Loader::Forge, 300));
        assert_eq!(java25.collector, Collector::Zgc);
        assert!(!java25.args().iter().any(|a| a.contains("ZGenerational")));
    }

    #[test]
    fn manual_choices_are_respected_with_warnings() {
        let pc = machine(16, 8);
        let settings = Settings {
            memory_mb: Some(14 * 1024),
            gc: GcChoice::Zgc,
            ..Default::default()
        };
        let plan = plan(&pc, &work(17, Loader::Fabric, 10), &settings);
        assert_eq!(plan.heap_mb, 14 * 1024);
        assert_eq!(plan.collector, Collector::Zgc);
        // 超过建议上限、Java 17 的 ZGC 不分代、此刻空闲内存不够。
        assert_eq!(plan.warnings.len(), 3);
        let old = plan_for_gc(&pc, &work(8, Loader::Vanilla, 0), GcChoice::Zgc);
        assert_eq!(old.collector, Collector::G1);
    }

    fn plan_for_gc(machine: &Machine, work: &Workload, gc: GcChoice) -> Plan {
        plan(
            machine,
            work,
            &Settings {
                gc,
                ..Default::default()
            },
        )
    }

    #[test]
    fn newer_java_only_gets_product_flags() {
        let plan = auto(&machine(16, 8), &work(30, Loader::Vanilla, 0));
        let args = plan.args();
        assert!(args.contains(&"-XX:+UseG1GC".to_string()));
        assert!(
            !args
                .iter()
                .any(|a| a.contains("Unlock") || a.contains("NewSize"))
        );
        assert!(!args.iter().any(|a| a.contains("CompactObjectHeaders")));
        let java25 = auto(&machine(16, 8), &work(25, Loader::Vanilla, 0)).args();
        assert!(java25.contains(&"-XX:+UseCompactObjectHeaders".to_string()));
    }

    #[test]
    fn user_arguments_win() {
        let settings = Settings {
            extra: split_args("-Xmx6G -XX:+UseShenandoahGC -XX:+PerfDisableSharedMem"),
            ..Default::default()
        };
        let plan = plan(&machine(16, 8), &work(21, Loader::Vanilla, 0), &settings);
        let mut jvm = vec!["-Xss1M".to_string()];
        plan.apply(&mut jvm);
        assert_eq!(
            jvm,
            vec![
                "-Xss1M",
                "-XX:+UnlockExperimentalVMOptions",
                "-Xmx6G",
                "-XX:+UseShenandoahGC",
                "-XX:+PerfDisableSharedMem",
            ]
        );
    }

    #[test]
    fn unlock_stays_before_experimental_flags() {
        let settings = Settings {
            extra: vec!["-XX:+UnlockExperimentalVMOptions".into()],
            ..Default::default()
        };
        let args = plan(&machine(16, 8), &work(21, Loader::Vanilla, 0), &settings).args();
        let unlock = args
            .iter()
            .position(|a| a == "-XX:+UnlockExperimentalVMOptions")
            .unwrap();
        let new_size = args
            .iter()
            .position(|a| a.starts_with("-XX:G1NewSizePercent"))
            .unwrap();
        assert!(unlock < new_size);
    }

    #[test]
    fn splits_quoted_arguments() {
        assert_eq!(
            split_args(r#" -Dname="a b"  -Dx='' -Xmx4G "#),
            vec!["-Dname=a b", "-Dx=", "-Xmx4G"]
        );
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn unknown_machine_falls_back_to_old_default() {
        let plan = auto(&Machine::default(), &work(21, Loader::Forge, 300));
        assert_eq!(plan.heap_mb, UNKNOWN_MACHINE_MB);
    }

    #[test]
    fn loader_comes_from_the_main_class() {
        assert_eq!(
            Loader::from_main_class("cpw.mods.bootstraplauncher.BootstrapLauncher"),
            Loader::Forge
        );
        assert_eq!(
            Loader::from_main_class("net.fabricmc.loader.impl.launch.knot.KnotClient"),
            Loader::Fabric
        );
        assert_eq!(
            Loader::from_main_class("net.minecraft.launchwrapper.Launch"),
            Loader::Forge
        );
        assert_eq!(
            Loader::from_main_class("net.minecraft.client.main.Main"),
            Loader::Vanilla
        );
    }

    /// 用本机装的每个 Java 跑一遍生成的参数，确认 JVM 都认。
    #[test]
    #[ignore = "需要本机装有 Java"]
    fn generated_flags_start_every_installed_java() {
        for java in java::find_all() {
            for gc in GcChoice::ALL {
                let w = Workload {
                    java_major: java.major,
                    java_64bit: java.is_64bit(),
                    graal: java.is_graal(),
                    loader: Loader::Forge,
                    mods: 120,
                    official: Vec::new(),
                };
                let pc = Machine {
                    available_mb: Machine::detect().available_mb,
                    ..machine(16, 8)
                };
                let plan = plan_for_gc(&pc, &w, gc);
                let out = std::process::Command::new(&java.path)
                    .args(plan.args())
                    .arg("-version")
                    .output()
                    .unwrap();
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(out.status.success(), "{}: {stderr}", java.label());
                assert!(
                    !stderr.contains("warning") || stderr.contains("JVMCI"),
                    "{} {:?}: {stderr}",
                    java.label(),
                    gc
                );
            }
        }
    }
}
