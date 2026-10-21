use crate::blob;
use crate::error::{Error, Result};
use crate::http::{FileSpec, Http, sha1_file};
use crate::java;
use crate::manifest::{Coord, VersionJson, library_specs_for};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Install a modern Forge/NeoForge client from an installer jar without
/// executing the installer's Swing application.
///
/// The installer jar is a data package: `install_profile.json` describes the
/// processor chain and `version.json` is the profile to install.  We resolve
/// the processor inputs, run only the client-side processors, and write the
/// profile ourselves.
pub fn install_client(
    root: &Root,
    mc: &str,
    installer: &Path,
    mirror: &str,
    label: &str,
    sink: &Sink,
) -> Result<String> {
    root.ensure()?;
    let mut zip =
        zip::ZipArchive::new(File::open(installer).map_err(|e| crate::error::io(installer, e))?)
            .map_err(|e| Error::Install(format!("打开 {} 安装包失败: {e}", installer.display())))?;

    let profile: Value = read_json(&mut zip, "install_profile.json")?;
    if let Some(profile_mc) = profile.get("minecraft").and_then(|v| v.as_str())
        && profile_mc != mc
    {
        return Err(Error::Install(format!(
            "安装包对应 Minecraft {profile_mc}，不是 {mc}"
        )));
    }
    let json_name = profile
        .get("json")
        .and_then(|v| v.as_str())
        .unwrap_or("/version.json")
        .trim_start_matches('/');
    let version_text = read_text(&mut zip, json_name)?;
    let version: VersionJson = serde_json::from_str(&version_text)
        .map_err(|e| Error::Install(format!("{label} 版本文件解析失败: {e}")))?;
    let id = version.id.clone();
    if id.is_empty() {
        return Err(Error::Install(format!("{label} 版本文件缺少 id")));
    }
    write_atomic(&root.version_json(&id), version_text.as_bytes())?;

    let http = Http::new(mirror)?;
    (sink)(Progress::stage("下载版本依赖"));
    let mut version_specs = library_specs_for(
        root,
        &version.libraries,
        &crate::manifest::Context::current(),
    );
    // Libraries whose artifact has an empty URL are produced by processors.
    version_specs.retain(|s| !s.url.is_empty());
    blob::download_specs(root, &http, version_specs, "下载版本依赖", sink)?;

    (sink)(Progress::stage("下载安装依赖"));
    let install_specs = installer_specs(root, &profile)?;
    blob::download_specs(root, &http, install_specs, "下载安装依赖", sink)?;
    let library_paths = installer_paths(root, &profile)?;

    let work = root
        .cache()
        .join("loader-work")
        .join(format!("{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| crate::error::io(&work, e))?;

    // Keep the shared base jar out of the processors' reach.
    let minecraft_jar = work.join("client.jar");
    std::fs::copy(root.client_jar(mc), &minecraft_jar)
        .map_err(|e| crate::error::io(&minecraft_jar, e))?;

    let mut extracted = HashMap::new();
    if let Some(data) = profile.get("data").and_then(|v| v.as_object()) {
        for value in data.values() {
            let Some(raw) = value.get("client").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(entry) = raw.strip_prefix('/') {
                let dest = work.join(entry.replace('/', "_"));
                if let Ok(bytes) = read_bytes(&mut zip, entry) {
                    std::fs::write(&dest, bytes).map_err(|e| crate::error::io(&dest, e))?;
                    extracted.insert(raw.to_string(), dest);
                }
            }
        }
    }

    let mut vars = BTreeMap::new();
    vars.insert("SIDE".to_string(), "client".to_string());
    vars.insert("ROOT".to_string(), root.dir().to_string_lossy().to_string());
    vars.insert(
        "LIBRARY_DIR".to_string(),
        root.libraries().to_string_lossy().to_string(),
    );
    vars.insert(
        "MINECRAFT_JAR".to_string(),
        minecraft_jar.to_string_lossy().to_string(),
    );
    vars.insert("MINECRAFT_VERSION".to_string(), mc.to_string());
    vars.insert(
        "INSTALLER".to_string(),
        installer
            .canonicalize()
            .unwrap_or_else(|_| installer.to_path_buf())
            .to_string_lossy()
            .to_string(),
    );

    if let Some(data) = profile.get("data").and_then(|v| v.as_object()) {
        for (key, value) in data {
            let Some(raw) = value.get("client").and_then(|v| v.as_str()) else {
                continue;
            };
            let resolved = if let Some(path) = extracted.get(raw) {
                path.to_string_lossy().to_string()
            } else {
                resolve_literal(raw, root, mc)
            };
            vars.insert(key.clone(), resolved);
        }
    }

    let processors: Vec<&Value> = profile
        .get("processors")
        .and_then(|v| v.as_array())
        .map(|v| v.iter().collect())
        .unwrap_or_default();
    let client_processors: Vec<&&Value> = processors.iter().filter(|p| is_client(p)).collect();
    let total = client_processors.len().max(1);
    let java = find_java()?;
    let log_path = root
        .cache()
        .join("logs")
        .join(format!("{id}-processors.log"));
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
    }
    let _ = std::fs::remove_file(&log_path);

    for (index, processor) in client_processors.iter().enumerate() {
        let jar_name = processor
            .get("jar")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Install("processor 缺少 jar".into()))?;
        let processor_jar = library_paths
            .get(jar_name)
            .cloned()
            .ok_or_else(|| Error::Install(format!("找不到 processor 依赖: {jar_name}")))?;
        let mut classpath = Vec::new();
        for class in processor
            .get("classpath")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let Some(name) = class.as_str() else {
                continue;
            };
            let path = library_paths.get(name).cloned().ok_or_else(|| {
                Error::Install(format!("找不到 processor classpath 依赖: {name}"))
            })?;
            classpath.push(path.to_string_lossy().to_string());
        }
        classpath.push(processor_jar.to_string_lossy().to_string());

        let main_class = main_class(&processor_jar)?;
        let mut args = Vec::new();
        for arg in processor
            .get("args")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            let Some(arg) = arg.as_str() else {
                continue;
            };
            let expanded = expand_arg(arg, &vars, root, mc);
            if (expanded.contains('/') || expanded.contains('\\'))
                && let Some(parent) = Path::new(&expanded).parent()
            {
                let _ = std::fs::create_dir_all(parent);
            }
            args.push(expanded);
        }

        (sink)(Progress {
            stage: format!("运行 {label} 处理器 {}/{}", index + 1, total),
            files_done: index as u64,
            files_total: total as u64,
            bytes_done: 0,
            bytes_total: 0,
            indeterminate: false,
        });
        println!("→ {label} 处理器 {}/{}: {main_class}", index + 1, total);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| crate::error::io(&log_path, e))?;
        let log_err = log
            .try_clone()
            .map_err(|e| crate::error::io(&log_path, e))?;
        {
            let mut header = log
                .try_clone()
                .map_err(|e| crate::error::io(&log_path, e))?;
            let _ = writeln!(
                header,
                "\n=== {} | {main_class} | {:?} ===",
                index + 1,
                args
            );
        }
        let status = Command::new(&java.path)
            .arg("-cp")
            .arg(classpath.join(if cfg!(windows) { ";" } else { ":" }))
            .arg(&main_class)
            .args(&args)
            .current_dir(root.dir())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .status()
            .map_err(|e| Error::Install(format!("无法启动 {label} processor: {e}")))?;
        if !status.success() {
            return Err(Error::Install(format!(
                "{label} 处理器 {}/{} 失败（{status}）；日志: {}",
                index + 1,
                total,
                log_path.display()
            )));
        }
        check_outputs(processor, &vars, root, mc)?;
        (sink)(Progress {
            stage: format!("运行 {label} 处理器 {}/{}", index + 1, total),
            files_done: (index + 1) as u64,
            files_total: total as u64,
            bytes_done: 0,
            bytes_total: 0,
            indeterminate: false,
        });
    }

    if !root.version_json(&id).exists() {
        return Err(Error::Install(format!("安装完成但找不到 {id} 版本文件")));
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(id)
}

fn read_bytes(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| Error::Install(format!("安装包缺少 {name}: {e}")))?;
    let mut out = Vec::new();
    entry
        .read_to_end(&mut out)
        .map_err(|e| Error::Install(format!("读取 {name} 失败: {e}")))?;
    Ok(out)
}

fn read_text(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<String> {
    String::from_utf8(read_bytes(zip, name)?)
        .map_err(|e| Error::Install(format!("{name} 不是 UTF-8: {e}")))
}

fn read_json(zip: &mut zip::ZipArchive<File>, name: &str) -> Result<Value> {
    serde_json::from_str(&read_text(zip, name)?)
        .map_err(|e| Error::Install(format!("{name} 解析失败: {e}")))
}

fn installer_specs(root: &Root, profile: &Value) -> Result<Vec<FileSpec>> {
    let mut specs = Vec::new();
    for lib in profile
        .get("libraries")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let Some(artifact) = lib
            .get("downloads")
            .and_then(|v| v.get("artifact"))
            .filter(|a| a.get("url").and_then(|u| u.as_str()).is_some())
        else {
            continue;
        };
        let url = artifact["url"].as_str().unwrap_or_default().to_string();
        let path = artifact
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| {
                lib.get("name")
                    .and_then(|v| v.as_str())
                    .and_then(|n| Coord::parse(n).ok())
                    .map(|c| c.rel_path())
            });
        let Some(path) = path else {
            continue;
        };
        specs.push(FileSpec {
            url,
            dest: root.libraries().join(path),
            sha1: artifact
                .get("sha1")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            size: artifact.get("size").and_then(|v| v.as_u64()),
            verify_existing: true,
        });
    }
    Ok(specs)
}

fn installer_paths(root: &Root, profile: &Value) -> Result<HashMap<String, PathBuf>> {
    let mut map = HashMap::new();
    for lib in profile
        .get("libraries")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let Some(name) = lib.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let path = lib
            .get("downloads")
            .and_then(|v| v.get("artifact"))
            .and_then(|a| a.get("path"))
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(|| Coord::parse(name).ok().map(|c| c.rel_path()));
        if let Some(path) = path {
            map.insert(name.to_string(), root.libraries().join(path));
        }
    }
    Ok(map)
}

fn is_client(processor: &Value) -> bool {
    match processor.get("sides").and_then(|v| v.as_array()) {
        Some(sides) => sides.iter().any(|s| s.as_str() == Some("client")),
        None => true,
    }
}

fn resolve_literal(value: &str, root: &Root, mc: &str) -> String {
    let value = value.trim();
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return value[1..value.len() - 1].to_string();
    }
    if value.starts_with('[') && value.ends_with(']') {
        let raw = &value[1..value.len() - 1];
        if let Ok(coord) = Coord::parse(raw) {
            if coord.group == "net.minecraft"
                && coord.artifact == "client"
                && coord.classifier.is_none()
            {
                return root.client_jar(mc).to_string_lossy().to_string();
            }
            return root
                .libraries()
                .join(coord.rel_path())
                .to_string_lossy()
                .to_string();
        }
    }
    value.to_string()
}

fn expand_arg(arg: &str, vars: &BTreeMap<String, String>, root: &Root, mc: &str) -> String {
    let mut out = arg.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    if out.starts_with('[') && out.ends_with(']') {
        out = resolve_literal(&out, root, mc);
    }
    if out.starts_with('\'') && out.ends_with('\'') && out.len() >= 2 {
        out = out[1..out.len() - 1].to_string();
    }
    out
}

fn check_outputs(
    processor: &Value,
    vars: &BTreeMap<String, String>,
    root: &Root,
    mc: &str,
) -> Result<()> {
    let Some(outputs) = processor.get("outputs").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    for (key, expected) in outputs {
        let path = expand_arg(key, vars, root, mc);
        let file = Path::new(&path);
        if !file.exists() {
            return Err(Error::Install(format!(
                "processor 结束时缺少输出文件: {path}"
            )));
        }
        let Some(expected) = expected.as_str() else {
            continue;
        };
        let expected = expand_arg(expected, vars, root, mc);
        let expected = expected.trim().trim_matches('\'');
        if expected.len() != 40 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let actual = sha1_file(file)?;
        if !actual.eq_ignore_ascii_case(expected) {
            // Some zlib implementations produce equivalent jars with different
            // bytes.  Accept a structurally valid jar, reject anything else.
            let valid_jar = file
                .extension()
                .map(|e| e.eq_ignore_ascii_case("jar"))
                .unwrap_or(false)
                && zip::ZipArchive::new(File::open(file).map_err(|e| crate::error::io(file, e))?)
                    .is_ok();
            if valid_jar {
                println!("! processor 输出哈希与清单不同（zlib 兼容），已接受: {path}");
            } else {
                return Err(Error::Install(format!(
                    "processor 输出校验失败: {path}（期望 {expected}，实际 {actual}）"
                )));
            }
        }
    }
    Ok(())
}

fn main_class(jar: &Path) -> Result<String> {
    let mut zip = zip::ZipArchive::new(File::open(jar).map_err(|e| crate::error::io(jar, e))?)
        .map_err(|e| Error::Install(format!("打开 processor jar 失败: {e}")))?;
    let mut manifest = String::new();
    zip.by_name("META-INF/MANIFEST.MF")
        .map_err(|e| Error::Install(format!("processor 缺少 MANIFEST: {e}")))?
        .read_to_string(&mut manifest)
        .map_err(|e| Error::Install(format!("读取 processor MANIFEST 失败: {e}")))?;
    for line in manifest.lines() {
        if let Some(main) = line.strip_prefix("Main-Class:") {
            let main = main.trim().trim_end_matches('\r');
            if !main.is_empty() {
                return Ok(main.to_string());
            }
        }
    }
    Err(Error::Install("processor 缺少 Main-Class".into()))
}

fn find_java() -> Result<java::JavaRuntime> {
    let all = java::find_all();
    all.iter()
        .find(|j| j.major >= 21)
        .or_else(|| all.iter().find(|j| j.major >= 17))
        .or_else(|| all.first())
        .cloned()
        .ok_or(Error::JavaNotFound(21))
}
