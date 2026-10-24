use anyhow::Context;
use clap::{Parser, Subcommand};
use nailong_core::install::{fabric, forge, modpack, neoforge, vanilla};
use nailong_core::launch::{self, LaunchOptions};
use nailong_core::manifest::{self, Context as ResolveContext};
use nailong_core::progress::{Progress, Sink, from_fn};
use nailong_core::{Root, cancel, java};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(
    name = "nailong",
    version,
    about = "轻量的 Minecraft 启动器（neoForge 预览版）"
)]
struct Cli {
    /// 游戏数据目录（默认 ~/.local/share/nailong/.minecraft 或 %APPDATA%\\nailong\\.minecraft）
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
    /// 查看某个版本的解析结果
    Info { profile: String },
    /// 安装
    Install {
        #[command(subcommand)]
        target: InstallTarget,
    },
    /// 启动
    Launch {
        profile: String,
        #[arg(long, default_value = "Player")]
        username: String,
        #[arg(long)]
        uuid: Option<String>,
        #[arg(long, default_value_t = 4096)]
        memory: u32,
        #[arg(long, default_value_t = 1024)]
        min_memory: u32,
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = Root::new(cli.root.clone().unwrap_or_else(Root::default_root));
    println!(
        "nailong {} · 数据目录: {}",
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
        Commands::Java => {
            let all = java::find_all();
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
                width,
                height,
                demo,
                game_dir,
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
