use crate::blob;
use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::manifest::{Context, Coord, VersionJson, library_specs_for};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PatchedTarget {
    pub rel_path: PathBuf,
    /// Forge ships an output hash in `PATCHED_SHA`; NeoForge currently does
    /// not.  When absent, fall back to a structural client-jar check.
    pub sha1: Option<String>,
}

/// Read the final patched-client coordinate/hash from `install_profile.json`.
/// Forge and NeoForge both expose this as `data.PATCHED.client` plus
/// `data.PATCHED_SHA.client`.
pub fn patched_target(profile: &Value) -> Option<PatchedTarget> {
    let raw = profile
        .pointer("/data/PATCHED/client")
        .and_then(|v| v.as_str())?;
    let sha1 = profile
        .pointer("/data/PATCHED_SHA/client")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().trim_matches('\'').to_ascii_lowercase())
        .filter(|v| v.len() == 40 && v.chars().all(|c| c.is_ascii_hexdigit()));
    let coord = raw.trim().trim_start_matches('[').trim_end_matches(']');
    let coord = Coord::parse(coord).ok()?;
    Some(PatchedTarget {
        rel_path: coord.rel_path(),
        sha1,
    })
}

/// Try to materialise a patched client from the current root or another
/// launcher's instance.  Every candidate is verified by SHA1.
pub fn import_patched(root: &Root, target: &PatchedTarget) -> Result<Option<PathBuf>> {
    let dest = root.libraries().join(&target.rel_path);
    if dest.is_file() && patched_ok(&dest, target) {
        let _ = blob::store(root, &dest);
        return Ok(Some(dest));
    }
    if dest.exists() {
        let _ = std::fs::remove_file(&dest);
    }

    for candidate in candidate_roots(root) {
        let source = candidate.join("libraries").join(&target.rel_path);
        if !source.is_file() || !patched_ok(&source, target) {
            continue;
        }
        let sha1 = blob::store(root, &source)?;
        if blob::materialize(root, &sha1, &dest)? {
            if target.sha1.is_none() {
                println!("! NeoForge 未提供输出哈希，已按 JAR 结构校验后复用");
            }
            println!("→ 复用本机已有 patched client: {}", source.display());
            return Ok(Some(source));
        }
    }
    Ok(None)
}

fn patched_ok(path: &Path, target: &PatchedTarget) -> bool {
    if let Some(expected) = &target.sha1 {
        return blob::verify(path, expected).is_ok();
    }
    // NeoForge does not publish an output SHA1.  A real patched client is a
    // multi-megabyte readable zip containing the Minecraft main class.
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() < 1_000_000 {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return false;
    };
    zip.by_name("net/minecraft/client/main/Main.class").is_ok()
}

/// Write a loader's version JSON and install only its launch libraries.
/// This is the no-processor path used when a verified patched client already
/// exists locally.
pub fn install_version_profile(
    root: &Root,
    version_text: &str,
    mc: &str,
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    let version: VersionJson = serde_json::from_str(version_text)
        .map_err(|e| Error::Install(format!("加载器版本文件解析失败: {e}")))?;
    if version.id.is_empty() {
        return Err(Error::Install("加载器版本文件缺少 id".into()));
    }
    write_atomic(&root.version_json(&version.id), version_text.as_bytes())?;

    let specs: Vec<FileSpec> = library_specs_for(root, &version.libraries, &Context::current())
        .into_iter()
        .filter(|spec| !spec.url.is_empty())
        .collect();
    (sink)(Progress::stage("下载加载器运行库"));
    let http = Http::new(mirror)?;
    blob::download_specs(root, &http, specs, "下载加载器运行库", sink)?;
    println!("✓ 复用已有客户端，跳过 processor（{mc} / {}）", version.id);
    Ok(version.id)
}

fn candidate_roots(current: &Root) -> Vec<PathBuf> {
    let mut roots = vec![current.dir().to_path_buf()];
    if let Some(parent) = current.dir().parent() {
        roots.push(parent.join(".minecraft"));
    }
    if let Some(root) = official_root() {
        roots.push(root);
    }
    if let Some(dirs) = directories::BaseDirs::new() {
        roots.extend(nearby_roots(dirs.home_dir()));
        roots.extend(hmcl_config_roots(dirs.home_dir()));
        let data = dirs.data_dir();
        roots.extend(instance_roots(data.join("PrismLauncher").join("instances")));
        roots.extend(instance_roots(data.join("multimc").join("instances")));
        roots.extend(instance_roots(
            data.join("com.modrinth.theseus").join("profiles"),
        ));
        #[cfg(windows)]
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let appdata = PathBuf::from(appdata);
            roots.extend(instance_roots(
                appdata.join("PrismLauncher").join("instances"),
            ));
            roots.extend(instance_roots(appdata.join("MultiMC").join("instances")));
            roots.extend(instance_roots(
                appdata.join("com.modrinth.theseus").join("profiles"),
            ));
        }
        let _ = data;
    }
    if let Some(extra) = std::env::var_os("NAILONG_REUSE_ROOTS") {
        roots.extend(std::env::split_paths(&extra));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn instance_roots(base: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(base.as_ref()) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        out.push(entry.path().join(".minecraft"));
        out.push(entry.path().join("minecraft"));
    }
    out
}

fn nearby_roots(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for base in [
        home.to_path_buf(),
        home.join("Desktop"),
        home.join("Documents"),
        home.join("Downloads"),
    ] {
        out.push(base.join(".minecraft"));
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten().take(128) {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.contains("pcl")
                || name.contains("hmcl")
                || name.contains("minecraft")
                || name.contains("launcher")
            {
                out.push(entry.path().join(".minecraft"));
            }
        }
    }
    out
}

fn hmcl_config_roots(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut candidates = vec![home.join(".hmcl.json")];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(appdata).join(".hmcl.json"));
    }
    for path in candidates {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        for key in ["gameDir", "gamePath", "minecraftDir"] {
            if let Some(dir) = value.get(key).and_then(|v| v.as_str()) {
                out.push(PathBuf::from(dir));
            }
        }
    }
    out
}

fn official_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join(".minecraft"))
    }
    #[cfg(not(windows))]
    {
        directories::BaseDirs::new().map(|d| d.home_dir().join(".minecraft"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_patched_target_from_forge_profile() {
        let profile = json!({
            "data": {
                "PATCHED": {"client": "[net.minecraftforge:forge:1.21.1-52.1.20:client]"},
                "PATCHED_SHA": {"client": "'28f7acaa0e9b1c2d2a7bf52c07ae16133bd3da4e'"}
            }
        });
        let target = patched_target(&profile).unwrap();
        assert_eq!(
            target.rel_path,
            PathBuf::from(
                "net/minecraftforge/forge/1.21.1-52.1.20/forge-1.21.1-52.1.20-client.jar"
            )
        );
        assert_eq!(
            target.sha1.as_deref(),
            Some("28f7acaa0e9b1c2d2a7bf52c07ae16133bd3da4e")
        );
    }

    #[test]
    fn rejects_bad_hash() {
        let profile = json!({
            "data": {
                "PATCHED": {"client": "[net.neoforged:neoforge:21.1.209:client]"},
                "PATCHED_SHA": {"client": "nope"}
            }
        });
        let target = patched_target(&profile).unwrap();
        assert!(target.sha1.is_none());
    }
}
