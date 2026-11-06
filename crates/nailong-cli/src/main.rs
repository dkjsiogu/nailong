use anyhow::Context;
use clap::{Parser, Subcommand};
use nailong_core::install::{fabric, forge, modpack, neoforge, vanilla};
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::manifest::{self, Context as ResolveContext};
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::{Root, bench, cancel, java, tuning};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(
    name = "cairn-cli",
    version,
    about = "轻量的 Minecraft 启动器（neoForge 预览版）"
)]
struct Cli {
    /// 游戏数据目录（默认 ~/.local/share/cairn/.minecraft；Windows 默认在程序旁边的 .minecraft）
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// 下载源：official | bmcl（国内镜像）
    #[arg(long, global = true, default_value = "official")]
    mirror: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 列出已安装的版本
    List,
    /// 列出检测到的 Java
    Java,
    /// 下载官方的 Java 运行时到数据目录（例如 21、17、8）
    JavaInstall { major: u32 },
    /// 查一下有没有新版本的 Cairn
    Update,
    /// 查看某个版本的解析结果
    Info { profile: String },
    /// 安装
    Install {
        #[command(subcommand)]
        target: InstallTarget,
    },
    /// 测一下这台电脑：用模拟游戏负载在每个 Java 21+ 上比较 G1 和 ZGC，所有版本共用结论
    Bench,
    /// 启动
    Launch {
        profile: String,
        #[arg(long, default_value = "Player")]
        username: String,
        #[arg(long)]
        uuid: Option<String>,
        /// 最大堆（MB）；不填时按电脑配置和 Mod 数量自动算
        #[arg(long)]
        memory: Option<u32>,
        #[arg(long)]
        min_memory: Option<u32>,
        /// 回收器：auto、g1 或 zgc
        #[arg(long, default_value = "auto")]
        gc: String,
        /// 额外的 JVM 参数，可重复；和自动参数冲突时以它为准
        #[arg(long = "jvm-arg", allow_hyphen_values = true)]
        jvm_args: Vec<String>,
        #[arg(long, default_value_t = 1280)]
        width: u32,
        #[arg(long, default_value_t = 720)]
        height: u32,
        #[arg(long)]
        demo: bool,
        #[arg(long)]
        game_dir: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        print_plan: bool,
    },
}

#[derive(Subcommand, Debug)]
enum InstallTarget {
    /// 原版
    Vanilla { mc: String },
    /// Fabric（原版 + Fabric profile）
    Fabric { mc: String, loader: String },
    /// NeoForge（先安装原版，再执行客户端 processors）
    Neoforge { mc: String, version: String },
    /// Forge（先安装原版，再执行客户端 processors）
    Forge { mc: String, version: String },
    /// 整合包（Modrinth / CurseForge / Prism / HMCL / ATLauncher / MCBBS）
    Modpack { path: PathBuf },
}

/// 和界面一样：版本做了隔离就用版本目录，否则用数据目录。
fn default_game_dir(root: &Root, profile: &str) -> PathBuf {
    if root.isolated_marker(profile).exists() {
        root.version_dir(profile)
    } else {
        root.dir().to_path_buf()
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = Root::new(cli.root.clone().unwrap_or_else(Root::default_root));
    root.migrate_legacy_cache();
    println!(
        "Cairn {} · 数据目录: {}",
        env!("CARGO_PKG_VERSION"),
        root.dir().display()
    );

    match cli.command {
        Commands::List => {
            let versions = root.versions();
            if !versions.exists() {
                println!("（还没有安装任何版本）");
                return Ok(());
            }
            let mut ids = Vec::new();
            for e in std::fs::read_dir(&versions)
                .with_context(|| format!("读取 {}", versions.display()))?
            {
                let e = e?;
                if e.path().is_dir() {
                    let name = e.file_name().to_string_lossy().to_string();
                    if root.version_json(&name).exists() {
                        ids.push(name);
                    }
                }
            }
            ids.sort();
            for id in ids {
                println!("  ● {id}");
            }
        }
        Commands::Bench => {
            let last = Arc::new(Mutex::new(String::new()));
            let sink = from_fn(move |p: Progress| {
                let line = format!("  {} {:>3}%", p.stage, (p.fraction() * 100.0) as u32);
                let mut last = last.lock().unwrap();
                if *last != line {
                    println!("{line}");
                    *last = line;
                }
            });
            bench::run(&root, &sink)?;
            let store = bench::load(&root).context("读不回刚保存的测试结果")?;
            for java in &store.javas {
                println!("\n{}", java.label());
                println!(
                    "  {:<12} {:<4} {:>7} {:>7} {:>8} {:>9} {:>9} {:>6}",
                    "档位", "回收", "堆", "平均帧", "1%低帧", "顿/分钟", "tick P95", "CPU核"
                );
                for tier in &java.tiers {
                    for run in [&tier.g1, &tier.zgc] {
                        println!(
                            "  {:<12} {:<4} {:>7} {:>7.0} {:>8.0} {:>9.1} {:>7.1}ms {:>6}",
                            tier.label,
                            run.collector.label(),
                            tuning::gb(run.heap_mb),
                            run.fps,
                            run.low_fps,
                            run.hitches_per_min,
                            run.tick_p95_ms,
                            run.cpu_cores
                                .map(|c| format!("{c:.1}"))
                                .unwrap_or_else(|| "-".into())
                        );
                    }
                    println!(
                        "  → {}档：一般版本 {}，官方默认 ZGC 的版本 {}",
                        tier.label,
                        tier.collector.label(),
                        tier.official.label()
                    );
                }
                for note in &java.notes {
                    println!("  {note}");
                }
            }
            for note in &store.skipped {
                println!("  跳过 {note}");
            }
            println!("\n✓ 结论已保存，所有版本的「自动」回收器按它选");
        }
        Commands::JavaInstall { major } => {
            let http = nailong_core::http::Http::new(&cli.mirror)?;
            let offer = nailong_core::runtime::find(&http, major, None)?;
            println!(
                "→ 下载 Java {}（{} {}）",
                offer.major, offer.component, offer.version
            );
            let last = Arc::new(Mutex::new(String::new()));
            let sink = from_fn(move |p: Progress| {
                let line = format!(
                    "  {} {:>3}%{}",
                    p.stage,
                    (p.fraction() * 100.0) as u32,
                    p.source.map(|s| format!(" · {s}")).unwrap_or_default()
                );
                let mut last = last.lock().unwrap();
                if *last != line {
                    println!("{line}");
                    *last = line;
                }
            });
            let java = nailong_core::runtime::install(&root, &http, &offer, &sink)?;
            println!("✓ 已装好：{}", java.display());
        }
        Commands::Update => {
            let http = nailong_core::http::Http::new("official")?;
            let release = nailong_core::update::latest(&http)?;
            let current = nailong_core::update::current();
            if nailong_core::update::is_newer(&release.version, current) {
                println!(
                    "有新版本 {}（现在是 {current}）：{}",
                    release.version, release.page
                );
            } else {
                println!("已经是最新版 {current}（GitHub 最新 {}）", release.version);
            }
        }
        Commands::Java => {
            let all = java::find_all(&root);
            if all.is_empty() {
                println!("没有检测到 Java");
            }
            for j in all {
                println!(
                    "  Java {:<3} {:<28} {}",
                    j.major,
                    j.version,
                    j.path.display()
                );
            }
        }
        Commands::Info { profile } => {
            let ctx = ResolveContext::current();
            let r = manifest::resolve(&root, &profile, &ctx)?;
            println!("版本: {}", r.id);
            println!("继承链: {}", r.chain.join(" -> "));
            println!("主类: {}", r.main_class);
            println!("Java: {:?}", r.java_major);
            println!("客户端: {}", r.client_jar.display());
            println!("库: {}", r.libraries.len());
            println!(
                "资源索引: {:?}",
                r.asset_index.as_ref().map(|a| a.id.clone())
            );
        }
        Commands::Install { target } => {
            cancel::reset();
            let sink = cli_progress();
            match target {
                InstallTarget::Vanilla { mc } => {
                    vanilla::install_with(&root, &mc, &cli.mirror, &sink)?;
                }
                InstallTarget::Fabric { mc, loader } => {
                    fabric::install_with(&root, &mc, &loader, &cli.mirror, &sink)?;
                }
                InstallTarget::Neoforge { mc, version } => {
                    neoforge::install_with(&root, &mc, &version, &cli.mirror, &sink)?;
                }
                InstallTarget::Forge { mc, version } => {
                    forge::install_with(&root, &mc, &version, &cli.mirror, &sink)?;
                }
                InstallTarget::Modpack { path } => {
                    modpack::import_pack(&root, &path, &cli.mirror, &sink)?;
                }
            }
        }
        Commands::Launch {
            profile,
            username,
            uuid,
            memory,
            min_memory,
            gc,
            jvm_args,
            width,
            height,
            demo,
            game_dir,
            dry_run,
            print_plan,
        } => {
            let opts = LaunchOptions {
                username,
                uuid,
                memory_mb: memory,
                min_memory_mb: min_memory,
                gc: tuning::GcChoice::from_key(&gc),
                jvm_args,
                width,
                height,
                demo,
                game_dir: Some(game_dir.unwrap_or_else(|| default_game_dir(&root, &profile))),
                dry_run,
                print_plan,
                ..Default::default()
            };
            let code = launch::launch(&root, &profile, &opts)?;
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
    Ok(())
}

fn cli_progress() -> Sink {
    let last = Arc::new(Mutex::new((String::new(), u32::MAX)));
    from_fn(move |p: Progress| {
        let bucket = if p.indeterminate {
            0
        } else {
            (p.fraction() * 100.0) as u32 / 5
        };
        if let Ok(mut last) = last.lock()
            && (last.0 != p.stage || last.1 != bucket)
        {
            *last = (p.stage.clone(), bucket);
            if p.indeterminate {
                eprintln!("  {}", p.stage);
            } else {
                eprintln!("  {} {}%", p.stage, (p.fraction() * 100.0) as u32);
            }
        }
    })
}
