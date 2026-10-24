use crate::error::{Error, Result, io};
use crate::java;
use crate::manifest::{self, Argument, Context, Resolved, StringOrList};
use crate::paths::Root;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub username: String,
    pub uuid: Option<String>,
    /// Microsoft account access token; None keeps the offline placeholder `0`.
    pub access_token: Option<String>,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
    pub memory_mb: u32,
    pub min_memory_mb: u32,
    pub width: u32,
    pub height: u32,
    pub demo: bool,
    pub game_dir: Option<PathBuf>,
    pub dry_run: bool,
    pub print_plan: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            username: "Player".into(),
            uuid: None,
            access_token: None,
            xuid: None,
            client_id: None,
            memory_mb: 4096,
            min_memory_mb: 1024,
            width: 1280,
            height: 720,
            demo: false,
            game_dir: None,
            dry_run: false,
            print_plan: false,
        }
    }
}

pub fn launch(root: &Root, profile: &str, opts: &LaunchOptions) -> Result<i32> {
    let mut ctx = Context::current();
    ctx.features.custom_resolution = true;
    ctx.features.demo = opts.demo;
    let resolved = manifest::resolve(root, profile, &ctx)?;
    let required = resolved
        .java_major
        .unwrap_or_else(|| guess_java(&resolved.id));
    let javas = java::find_all();
    let java_rt = java::select(required, &javas)?;
    println!(
        "→ 版本 {}，使用 {}（需要 Java {required}）",
        resolved.id,
        java_rt.label()
    );

    let game_dir = opts
        .game_dir
        .clone()
        .unwrap_or_else(|| root.dir().to_path_buf());
    std::fs::create_dir_all(&game_dir).map_err(|e| io(&game_dir, e))?;

    let natives = prepare_natives(root, &resolved, &ctx)?;
    let asset_id = resolved
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "legacy".into());
    let uuid = opts
        .uuid
        .clone()
        .unwrap_or_else(|| offline_uuid(&opts.username));
    let memory = opts.memory_mb.max(1024);
    let min_memory = opts.min_memory_mb.min(memory);

    let mut classpath = manifest::classpath(root, &resolved, &ctx);
    // 1.17+ 的 Forge/NeoForge 由 BootstrapLauncher/ModLauncher 自己发现
    // patched client jar 与 universal jar；如果 classpath 再带游戏 jar，
    // 会出现 minecraft / _1._21._1 / 两个 neoforge 模块冲突。
    if resolved.main_class.contains("bootstraplauncher")
        || resolved.main_class.contains("modlauncher")
    {
        classpath.retain(|p| p != &resolved.client_jar);
    }

    let mut vars = BTreeMap::new();
    let access_token = opts
        .access_token
        .clone()
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| "0".to_string());
    vars.insert("auth_player_name".to_string(), opts.username.clone());
    vars.insert("auth_uuid".to_string(), uuid.clone());
    vars.insert("auth_access_token".to_string(), access_token.clone());
    vars.insert(
        "auth_session".to_string(),
        format!("token:{access_token}:{uuid}"),
    );
    vars.insert(
        "clientid".to_string(),
        opts.client_id.clone().unwrap_or_default(),
    );
    vars.insert(
        "auth_xuid".to_string(),
        opts.xuid.clone().unwrap_or_default(),
    );
    vars.insert("user_type".to_string(), "msa".to_string());
    vars.insert("user_properties".to_string(), "{}".to_string());
    vars.insert("version_name".to_string(), resolved.id.clone());
    vars.insert("version_type".to_string(), "release".to_string());
    vars.insert("profile_name".to_string(), resolved.id.clone());
    vars.insert(
        "game_directory".to_string(),
        game_dir.to_string_lossy().to_string(),
    );
    vars.insert(
        "game_assets".to_string(),
        root.assets().to_string_lossy().to_string(),
    );
    vars.insert(
        "assets_root".to_string(),
        root.assets().to_string_lossy().to_string(),
    );
    vars.insert("assets_index_name".to_string(), asset_id);
    vars.insert(
        "natives_directory".to_string(),
        natives.to_string_lossy().to_string(),
    );
    vars.insert(
        "library_directory".to_string(),
        root.libraries().to_string_lossy().to_string(),
    );
    vars.insert(
        "classpath_separator".to_string(),
        if cfg!(windows) {
            ";".into()
        } else {
            ":".into()
        },
    );
    vars.insert("launcher_name".to_string(), "nailong".to_string());
    vars.insert(
        "launcher_version".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    vars.insert(
        "primary_jar".to_string(),
        resolved.client_jar.to_string_lossy().to_string(),
    );
    vars.insert(
        "primary_jar_name".to_string(),
        resolved
            .client_jar
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    vars.insert("resolution_width".to_string(), opts.width.to_string());
    vars.insert("resolution_height".to_string(), opts.height.to_string());
    vars.insert("classpath".to_string(), join_paths(&classpath));
    vars.insert("quickPlayPath".into(), String::new());
    vars.insert("quickPlaySingleplayer".into(), String::new());
    vars.insert("quickPlayMultiplayer".into(), String::new());
    vars.insert("quickPlayRealms".into(), String::new());

    let mut jvm = expand_args(&resolved.arguments.jvm, &ctx);
    // 去掉 manifest 里的 -cp/classpath，由我们统一追加
    remove_classpath_args(&mut jvm);
    push_default(&mut jvm, "-Dfile.encoding=", "-Dfile.encoding=UTF-8");
    push_default(
        &mut jvm,
        "-Dlog4j2.formatMsgNoLookups=",
        "-Dlog4j2.formatMsgNoLookups=true",
    );
    push_default(
        &mut jvm,
        "-Dminecraft.launcher.brand=",
        "-Dminecraft.launcher.brand=nailong",
    );
    push_default(
        &mut jvm,
        "-Dminecraft.launcher.version=",
        &format!("-Dminecraft.launcher.version={}", env!("CARGO_PKG_VERSION")),
    );
    push_default(
        &mut jvm,
        "-Dminecraft.client.jar=",
        &format!("-Dminecraft.client.jar={}", resolved.client_jar.display()),
    );
    push_default(&mut jvm, "-Xms", &format!("-Xms{min_memory}m"));
    push_default(&mut jvm, "-Xmx", &format!("-Xmx{memory}m"));
    if let Some(logging) = &resolved.logging {
        let p = root.assets().join("log_configs").join(&logging.file.id);
        if p.exists() {
            let arg = logging.argument.replace("${path}", &p.to_string_lossy());
            push_default(&mut jvm, "-Dlog4j.configurationFile=", &arg);
        }
    }
    jvm = jvm.into_iter().map(|a| substitute(&a, &vars)).collect();

    let mut game = expand_args(&resolved.arguments.game, &ctx);
    if let Some(legacy) = &resolved.legacy_arguments {
        game.extend(legacy.split_whitespace().map(|s| s.to_string()));
    }
    if !game.iter().any(|a| a == "--username") {
        game.push("--username".into());
        game.push(opts.username.clone());
    }
    if !game.iter().any(|a| a == "--uuid") {
        game.push("--uuid".into());
        game.push(uuid.clone());
    }
    if !game.iter().any(|a| a == "--accessToken") {
        game.push("--accessToken".into());
        game.push(access_token.clone());
    }
    if !game.iter().any(|a| a == "--version") {
        game.push("--version".into());
        game.push(resolved.id.clone());
    }
    if !game.iter().any(|a| a == "--gameDir") {
        game.push("--gameDir".into());
        game.push(game_dir.to_string_lossy().to_string());
    }
    if !game.iter().any(|a| a == "--assetsDir") {
        game.push("--assetsDir".into());
        game.push(root.assets().to_string_lossy().to_string());
    }
    if opts.demo && !game.iter().any(|a| a == "--demo") {
        game.push("--demo".into());
    }
    game = game.into_iter().map(|a| substitute(&a, &vars)).collect();

    let mut argv: Vec<String> = vec![java_rt.path.to_string_lossy().to_string()];
    argv.extend(jvm.clone());
    argv.push("-cp".into());
    argv.push(join_paths(&classpath));
    argv.push(resolved.main_class.clone());
    argv.extend(game.clone());

    if opts.print_plan || opts.dry_run {
        println!("\nJava: {}", java_rt.path.display());
        println!("Classpath ({}):", classpath.len());
        for p in &classpath {
            println!("  {}", p.display());
        }
        println!("JVM:");
        for a in &jvm {
            println!("  {}", redact(a));
        }
        println!("Main: {}", resolved.main_class);
        println!("Game:");
        let mut i = 0;
        while i < game.len() {
            let a = &game[i];
            if a == "--accessToken" {
                println!("  --accessToken <redacted>");
                i += 2;
                continue;
            }
            println!("  {}", a);
            i += 1;
        }
        println!();
    }
    if opts.dry_run {
        return Ok(0);
    }

    println!("→ 启动游戏…（日志会输出到终端）");
    let log_path = root
        .cache()
        .join("logs")
        .join(format!("launch-{}.log", resolved.id));
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let log_file = std::fs::File::create(&log_path).map_err(|e| io(&log_path, e))?;
    let log = Arc::new(Mutex::new(log_file));

    let mut cmd = Command::new(&java_rt.path);
    cmd.args(&jvm)
        .arg("-cp")
        .arg(join_paths(&classpath))
        .arg(&resolved.main_class)
        .args(&game)
        .current_dir(&game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = std::env::var_os("NAILONG_KEEP_JAVA_OPTIONS") {
        let _ = env;
    } else {
        cmd.env_remove("_JAVA_OPTIONS");
        cmd.env_remove("JAVA_TOOL_OPTIONS");
        cmd.env_remove("JDK_JAVA_OPTIONS");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Launch(format!("无法启动 Java: {e}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let log1 = log.clone();
    let t1 = stdout.map(|s| std::thread::spawn(move || pump(s, log1)));
    let log2 = log.clone();
    let t2 = stderr.map(|s| std::thread::spawn(move || pump(s, log2)));
    let status = child
        .wait()
        .map_err(|e| Error::Launch(format!("等待游戏失败: {e}")))?;
    if let Some(t) = t1 {
        let _ = t.join();
    }
    if let Some(t) = t2 {
        let _ = t.join();
    }
    let code = status.code().unwrap_or(-1);
    if code == 0 {
        println!("✓ 游戏正常退出");
    } else {
        println!(
            "✗ 游戏异常退出（exit {code}），日志: {}",
            log_path.display()
        );
    }
    Ok(code)
}

fn pump<R: std::io::Read + Send + 'static>(reader: R, log: Arc<Mutex<std::fs::File>>) {
    let mut out = std::io::stdout();
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else {
            break;
        };
        let red = redact(&line);
        let _ = writeln!(out, "{red}");
        let _ = out.flush();
        if let Ok(mut f) = log.lock() {
            let _ = writeln!(f, "{red}");
        }
    }
}

fn join_paths(paths: &[PathBuf]) -> String {
    let sep = if cfg!(windows) { ";" } else { ":" };
    paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

fn redact(s: &str) -> String {
    // 简单脱敏：--accessToken 的值通常不会出现在游戏日志中；这里处理常见形式
    let mut out = s.to_string();
    if let Some(pos) = out.find("accessToken=") {
        let start = pos + "accessToken=".len();
        out.replace_range(start..out.len().min(start + 32), "***");
    }
    out
}

fn push_default(args: &mut Vec<String>, prefix: &str, value: &str) {
    if args.iter().any(|a| a == prefix || a.starts_with(prefix)) {
        return;
    }
    args.push(value.to_string());
}

fn remove_classpath_args(args: &mut Vec<String>) {
    let mut out = Vec::with_capacity(args.len());
    let mut skip = false;
    for a in args.drain(..) {
        if skip {
            skip = false;
            continue;
        }
        if a == "-cp" || a == "-classpath" || a == "--class-path" {
            skip = true;
            continue;
        }
        out.push(a);
    }
    *args = out;
}

fn expand_args(args: &[Argument], ctx: &Context) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(s) => out.push(s.clone()),
            Argument::Ruled { rules, value } => {
                if manifest::allowed(rules, ctx) {
                    match value {
                        StringOrList::One(s) => out.push(s.clone()),
                        StringOrList::Many(v) => out.extend(v.iter().cloned()),
                    }
                }
            }
        }
    }
    out
}

fn substitute(input: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = input.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
    }
    out
}

fn guess_java(profile: &str) -> u32 {
    if profile.starts_with("26.") {
        25
    } else if profile.starts_with("1.20.5")
        || profile.starts_with("1.20.6")
        || profile.starts_with("1.21")
    {
        21
    } else if profile.starts_with("1.18")
        || profile.starts_with("1.19")
        || profile.starts_with("1.20")
    {
        17
    } else if profile.starts_with("1.17") {
        16
    } else {
        8
    }
}

pub(crate) fn offline_uuid(name: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(format!("OfflinePlayer:{name}").as_bytes());
    let digest = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x30;
    b[8] = (b[8] & 0x3f) | 0x80;
    let u = uuid::Uuid::from_bytes(b);
    u.simple().to_string()
}

fn prepare_natives(root: &Root, resolved: &Resolved, ctx: &Context) -> Result<PathBuf> {
    let root_dir = root.natives(&resolved.id);
    let mut target = root_dir.clone();
    // 26.x: -Djava.library.path=${natives_directory}/java
    for arg in expand_args(&resolved.arguments.jvm, ctx) {
        if let Some(v) = arg.strip_prefix("-Djava.library.path=${natives_directory}") {
            if let Some(sub) = v.strip_prefix('/')
                && !sub.is_empty()
                && !sub.contains("..")
            {
                target = root_dir.join(sub.replace('/', std::path::MAIN_SEPARATOR_STR));
            }
            break;
        }
    }
    if target.exists() {
        let _ = std::fs::remove_dir_all(&target);
    }
    std::fs::create_dir_all(&target).map_err(|e| io(&target, e))?;
    let jars = manifest::native_jars(root, resolved, ctx);
    if jars.is_empty() {
        println!("→ 没有需要解压的 native 库");
        return Ok(target);
    }
    println!("→ 解压 native 库（{} 个 jar）…", jars.len());
    for jar in jars {
        let file = std::fs::File::open(&jar).map_err(|e| io(&jar, e))?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| Error::Install(format!("读取 native 失败 {}: {e}", jar.display())))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| Error::Install(e.to_string()))?;
            if entry.is_dir() {
                continue;
            }
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            let name_s = name.to_string_lossy().replace('\\', "/");
            if name_s.starts_with("META-INF/")
                || name_s.ends_with(".sha1")
                || name_s.ends_with(".git")
            {
                continue;
            }
            if name_s.contains("..") {
                continue;
            }
            let dest = target.join(&name_s);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
            }
            let mut out = std::fs::File::create(&dest).map_err(|e| io(&dest, e))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| io(&dest, e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if name_s.ends_with(".so") || name_s.ends_with(".dylib") {
                    let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classpath_args_are_removed_with_their_value() {
        let mut args = vec![
            "-Xmx2G".into(),
            "-cp".into(),
            "a.jar:b.jar".into(),
            "-Dfoo=bar".into(),
            "-classpath".into(),
            "c.jar".into(),
        ];
        remove_classpath_args(&mut args);
        assert_eq!(args, vec!["-Xmx2G", "-Dfoo=bar"]);
    }

    #[test]
    fn defaults_do_not_duplicate_existing_flags() {
        let mut args = vec!["-Xmx2G".into()];
        push_default(&mut args, "-Xmx", "-Xmx4G");
        push_default(&mut args, "-Xms", "-Xms1G");
        assert_eq!(args, vec!["-Xmx2G", "-Xms1G"]);
    }

    #[test]
    fn substitute_replaces_named_placeholders_only() {
        let vars = BTreeMap::from([
            ("a".to_string(), "1".to_string()),
            ("ab".to_string(), "2".to_string()),
        ]);
        assert_eq!(substitute("${a}-${ab}-${c}", &vars), "1-2-${c}");
    }

    #[test]
    fn java_guess_covers_modern_versions() {
        assert_eq!(guess_java("1.12.2"), 8);
        assert_eq!(guess_java("1.17.1"), 16);
        assert_eq!(guess_java("1.20.1"), 17);
        assert_eq!(guess_java("1.21.1"), 21);
        assert_eq!(guess_java("26.3"), 25);
    }

    #[test]
    fn offline_uuid_matches_java_name_uuid() {
        // Java: UUID.nameUUIDFromBytes("OfflinePlayer:Notch".getBytes(UTF_8))
        assert_eq!(offline_uuid("Notch"), "b50ad385829d3141a2167e7d7539ba7f");
        assert_eq!(offline_uuid("Player"), "a01e3843e5213998958af459800e4d11");
    }
}
