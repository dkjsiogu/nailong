//! Index of content files installed in one game directory.
//!
//! Minecraft only sees files in `mods/`, `resourcepacks/` and `shaderpacks/`.
//! The index lives next to those folders and is used by the GUI for
//! enable/disable, deletion, duplicate detection and update checks.

use crate::error::{Error, Result};
use crate::mods::{self, ContentKind, Loader, Source, Version};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const INDEX_FILE: &str = ".nailong-content.json";
const DISABLED_SUFFIX: &str = ".disabled";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledEntry {
    pub kind: ContentKind,
    /// Logical file name, e.g. `example.jar`.
    pub file: String,
    pub enabled: bool,
    pub name: String,
    pub mod_id: Option<String>,
    pub source: Option<Source>,
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub version_number: Option<String>,
    /// 上次扫描时磁盘上的大小和修改时间（Unix 纳秒）。都没变就不再打开压缩包读元数据，
    /// 几百个 Mod 的目录也只需要读一遍目录。旧索引没有这两项，第一次会完整扫一遍。
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub modified: Option<u64>,
}

/// 文件的大小和修改时间（纳秒），读不到时为 None。
fn stamp(meta: &fs::Metadata) -> (Option<u64>, Option<u64>) {
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64);
    (Some(meta.len()), modified)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContentIndex {
    #[serde(default)]
    pub entries: Vec<InstalledEntry>,
}

pub fn index_path(game_dir: &Path) -> PathBuf {
    game_dir.join(INDEX_FILE)
}

fn content_dir(game_dir: &Path, kind: ContentKind) -> PathBuf {
    game_dir.join(kind.folder())
}

/// 这一项在磁盘上的文件（禁用的带 `.disabled`）。
pub fn path_of(game_dir: &Path, entry: &InstalledEntry) -> PathBuf {
    actual_path(game_dir, entry)
}

fn actual_path(game_dir: &Path, entry: &InstalledEntry) -> PathBuf {
    let suffix = if entry.enabled { "" } else { DISABLED_SUFFIX };
    content_dir(game_dir, entry.kind).join(format!("{}{suffix}", entry.file))
}

/// 索引里的 `file` 只能是纯文件名。否则一份被改过的索引可以把删除/改名
/// 引到 game dir 之外（例如 `"file": "../../.bashrc"`）。
fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && Path::new(name).components().count() == 1
}

fn ensure_safe_entry(entry: &InstalledEntry) -> Result<()> {
    if is_safe_file_name(&entry.file) {
        Ok(())
    } else {
        Err(Error::Install(format!(
            "内容索引里的文件名不安全，已忽略: {}",
            entry.file
        )))
    }
}

pub fn load(game_dir: &Path) -> ContentIndex {
    let mut index: ContentIndex = fs::read(index_path(game_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    index.entries.retain(|entry| is_safe_file_name(&entry.file));
    index
}

pub fn save(game_dir: &Path, index: &ContentIndex) -> Result<()> {
    let path = index_path(game_dir);
    let bytes = serde_json::to_vec_pretty(index)
        .map_err(|e| Error::Install(format!("内容索引序列化失败: {e}")))?;
    crate::paths::write_atomic(&path, &bytes)
}

pub fn scan(game_dir: &Path, kind: ContentKind) -> Result<Vec<InstalledEntry>> {
    let mut index = load(game_dir);
    scan_into(&mut index, game_dir, kind)?;
    save(game_dir, &index)?;
    Ok(index
        .entries
        .into_iter()
        .filter(|entry| entry.kind == kind)
        .collect())
}

pub fn scan_all(game_dir: &Path) -> Result<ContentIndex> {
    let mut index = load(game_dir);
    for kind in [
        ContentKind::Mod,
        ContentKind::ResourcePack,
        ContentKind::Shader,
    ] {
        scan_into(&mut index, game_dir, kind)?;
    }
    save(game_dir, &index)?;
    Ok(index)
}

fn scan_into(index: &mut ContentIndex, game_dir: &Path, kind: ContentKind) -> Result<()> {
    let dir = content_dir(game_dir, kind);
    let mut seen = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for item in entries.flatten() {
            let path = item.path();
            let Ok(disk) = fs::metadata(&path) else {
                continue;
            };
            if !disk.is_file() {
                continue;
            }
            let raw = item.file_name().to_string_lossy().to_string();
            let (file, enabled) = raw
                .strip_suffix(DISABLED_SUFFIX)
                .map(|name| (name.to_string(), false))
                .unwrap_or((raw.clone(), true));
            if !is_content_file(&file, kind) {
                continue;
            }
            let (size, modified) = stamp(&disk);
            let existing = index
                .entries
                .iter()
                .find(|entry| entry.kind == kind && entry.file == file);
            // 大小和修改时间都对得上：文件没变，沿用上次读到的名字和 ID。
            if let Some(existing) = existing
                && existing.size.is_some()
                && existing.modified.is_some()
                && existing.size == size
                && existing.modified == modified
            {
                let mut entry = existing.clone();
                entry.enabled = enabled;
                seen.insert(file, entry);
                continue;
            }
            let meta = inspect_archive(&path, kind, &file);
            let entry = InstalledEntry {
                kind,
                file: file.clone(),
                enabled,
                // 从平台下载的保留平台上的名字；本地文件以文件里写的为准。
                name: existing
                    .filter(|entry| entry.source.is_some())
                    .map(|entry| entry.name.clone())
                    .unwrap_or(meta.name),
                mod_id: meta
                    .mod_id
                    .or_else(|| existing.and_then(|entry| entry.mod_id.clone())),
                source: existing.and_then(|entry| entry.source),
                project_id: existing.and_then(|entry| entry.project_id.clone()),
                version_id: existing.and_then(|entry| entry.version_id.clone()),
                version_number: existing.and_then(|entry| entry.version_number.clone()),
                size,
                modified,
            };
            seen.insert(file, entry);
        }
    }
    index
        .entries
        .retain(|entry| entry.kind != kind || seen.contains_key(&entry.file));
    for (file, entry) in seen {
        if let Some(existing) = index
            .entries
            .iter_mut()
            .find(|item| item.kind == kind && item.file == file)
        {
            *existing = entry;
        } else {
            index.entries.push(entry);
        }
    }
    Ok(())
}

fn is_content_file(file: &str, kind: ContentKind) -> bool {
    match kind {
        ContentKind::Mod => file.ends_with(".jar"),
        ContentKind::ResourcePack | ContentKind::Shader => {
            file.ends_with(".zip") || file.ends_with(".jar")
        }
    }
}

struct ArchiveMeta {
    name: String,
    mod_id: Option<String>,
}

fn inspect_archive(path: &Path, kind: ContentKind, fallback: &str) -> ArchiveMeta {
    let mut name = file_stem(fallback);
    let mut mod_id = None;
    if let Ok(file) = fs::File::open(path)
        && let Ok(mut zip) = zip::ZipArchive::new(file)
    {
        match kind {
            ContentKind::Mod => {
                if let Some(text) = read_entry(&mut zip, "fabric.mod.json")
                    && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                {
                    if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                        mod_id = Some(id.to_string());
                    }
                    if let Some(display) = value.get("name").and_then(|v| v.as_str()) {
                        name = display.to_string();
                    }
                } else if let Some(text) = read_entry(&mut zip, "META-INF/neoforge.mods.toml")
                    .or_else(|| read_entry(&mut zip, "META-INF/mods.toml"))
                {
                    if let Some(id) = toml_value(&text, "modId") {
                        mod_id = Some(id.clone());
                    }
                    if let Some(display) = toml_value(&text, "displayName") {
                        name = display;
                    }
                } else if let Some(text) = read_entry(&mut zip, "META-INF/MANIFEST.MF")
                    && let Some(title) = manifest_value(&text, "Implementation-Title")
                {
                    name = title;
                }
            }
            ContentKind::ResourcePack => {
                if let Some(text) = read_entry(&mut zip, "pack.mcmeta")
                    && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                    && let Some(description) = value.pointer("/pack/description")
                {
                    if let Some(text) = description.as_str() {
                        name = text.to_string();
                    } else if let Some(text) = description.get("text").and_then(|v| v.as_str()) {
                        name = text.to_string();
                    }
                }
            }
            ContentKind::Shader => {}
        }
    }
    ArchiveMeta { name, mod_id }
}

fn read_entry(zip: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<String> {
    let mut entry = zip.by_name(name).ok()?;
    let mut out = String::new();
    entry.read_to_string(&mut out).ok()?;
    Some(out)
}

fn toml_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((candidate, value)) = line.split_once('=') else {
            continue;
        };
        if candidate.trim() != key {
            continue;
        }
        let value = value.split('#').next()?.trim();
        return Some(value.trim_matches('"').trim_matches('\'').to_string());
    }
    None
}

fn manifest_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let Some((candidate, value)) = line.split_once(':') else {
            continue;
        };
        if candidate.trim() == key {
            return Some(value.trim().to_string());
        }
    }
    None
}

fn file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.to_string())
}

pub fn record_versions(
    game_dir: &Path,
    kind: ContentKind,
    versions: &[Version],
) -> Result<Vec<InstalledEntry>> {
    let mut index = load(game_dir);
    let dir = content_dir(game_dir, kind);
    for version in versions {
        let file = Path::new(&version.file_name)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or_else(|| Error::Install("非法内容文件名".into()))?;
        let path = dir.join(&file);
        if !path.is_file() {
            continue;
        }
        let meta = inspect_archive(&path, kind, &file);
        let entry = InstalledEntry {
            kind,
            file: file.clone(),
            enabled: true,
            name: meta.name,
            mod_id: meta.mod_id,
            source: Some(version.source),
            project_id: Some(version.project_id.clone()),
            version_id: Some(version.id.clone()),
            version_number: Some(version.version_number.clone()),
            size: None,
            modified: None,
        };
        if let Some(existing) = index
            .entries
            .iter_mut()
            .find(|item| item.kind == kind && item.file == file)
        {
            *existing = entry;
        } else {
            index.entries.push(entry);
        }
    }
    save(game_dir, &index)?;
    Ok(index
        .entries
        .into_iter()
        .filter(|entry| entry.kind == kind)
        .collect())
}

#[derive(Debug, Clone)]
pub struct UpdateCandidate {
    pub entry: InstalledEntry,
    pub latest: Version,
}

pub fn check_update(
    entry: &InstalledEntry,
    mc: &str,
    loader: Loader,
) -> Result<Option<UpdateCandidate>> {
    let (Some(source), Some(project_id)) = (entry.source, entry.project_id.as_deref()) else {
        return Ok(None);
    };
    let project = mods::project(source, project_id, entry.kind)?;
    let latest = mods::versions(&project, mc, loader)?.into_iter().next();
    Ok(latest.and_then(|latest| {
        if entry.version_id.as_deref() == Some(latest.id.as_str()) {
            None
        } else {
            Some(UpdateCandidate {
                entry: entry.clone(),
                latest,
            })
        }
    }))
}

pub fn set_enabled(game_dir: &Path, entry: &mut InstalledEntry, enabled: bool) -> Result<()> {
    ensure_safe_entry(entry)?;
    if entry.enabled == enabled {
        return Ok(());
    }
    let old = actual_path(game_dir, entry);
    let new = content_dir(game_dir, entry.kind).join(if enabled {
        entry.file.clone()
    } else {
        format!("{}{DISABLED_SUFFIX}", entry.file)
    });
    if !old.is_file() {
        return Err(Error::Install(format!("找不到文件: {}", old.display())));
    }
    if new.exists() {
        return Err(Error::Install(format!("目标已存在: {}", new.display())));
    }
    fs::rename(&old, &new).map_err(|e| crate::error::io(&new, e))?;
    entry.enabled = enabled;
    let mut index = load(game_dir);
    if let Some(item) = index
        .entries
        .iter_mut()
        .find(|item| item.kind == entry.kind && item.file == entry.file)
    {
        item.enabled = enabled;
    }
    save(game_dir, &index)
}

pub fn remove(game_dir: &Path, entry: &InstalledEntry) -> Result<()> {
    ensure_safe_entry(entry)?;
    let path = actual_path(game_dir, entry);
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| crate::error::io(&path, e))?;
    }
    let mut index = load(game_dir);
    index
        .entries
        .retain(|item| !(item.kind == entry.kind && item.file == entry.file));
    save(game_dir, &index)
}

pub fn duplicates(entries: &[InstalledEntry]) -> Vec<(String, Vec<InstalledEntry>)> {
    let mut groups: BTreeMap<String, Vec<InstalledEntry>> = BTreeMap::new();
    for entry in entries {
        let key = entry
            .project_id
            .as_ref()
            .map(|id| format!("{}:{id}", source_key(entry.source)))
            .or_else(|| entry.mod_id.clone());
        if let Some(key) = key {
            groups.entry(key).or_default().push(entry.clone());
        }
    }
    groups
        .into_iter()
        .filter(|(_, items)| items.len() > 1)
        .collect()
}

fn source_key(source: Option<Source>) -> &'static str {
    match source {
        Some(Source::Modrinth) => "modrinth",
        Some(Source::CurseForge) => "curseforge",
        None => "local",
    }
}

/// 拖进窗口的文件是什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    Content(ContentKind),
    Modpack,
    /// 认不出来的 zip：可能是资源包也可能是光影，由当前页面决定。
    Archive,
    Unknown,
}

/// 按扩展名和压缩包里的文件认出拖进来的东西。只读目录，不解压。
pub fn classify(path: &Path) -> Dropped {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jar" => return Dropped::Content(ContentKind::Mod),
        "mrpack" => return Dropped::Modpack,
        "zip" => {}
        _ => return Dropped::Unknown,
    }
    let Ok(file) = fs::File::open(path) else {
        return Dropped::Unknown;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return Dropped::Unknown;
    };
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let entry = zip.by_index_raw(i).ok()?;
            entry.name().ok().map(|name| name.to_string())
        })
        .collect();
    // 整合包：Modrinth、MCBBS、CurseForge、Prism/MultiMC 的清单都在根目录。
    if names.iter().any(|name| {
        matches!(
            name.as_str(),
            "modrinth.index.json"
                | "mcbbs.packmeta"
                | "manifest.json"
                | "mmc-pack.json"
                | "instance.cfg"
        )
    }) {
        return Dropped::Modpack;
    }
    if names.iter().any(|name| name.starts_with("shaders/")) {
        return Dropped::Content(ContentKind::Shader);
    }
    if names.iter().any(|name| name == "pack.mcmeta") {
        return Dropped::Content(ContentKind::ResourcePack);
    }
    Dropped::Archive
}

/// 这个文件（启用或禁用）是否已经在内容目录里。
pub fn contains(game_dir: &Path, kind: ContentKind, file: &str) -> bool {
    let dir = content_dir(game_dir, kind);
    dir.join(file).exists() || dir.join(format!("{file}{DISABLED_SUFFIX}")).exists()
}

pub fn import_local(game_dir: &Path, kind: ContentKind, source: &Path) -> Result<InstalledEntry> {
    let file = source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| Error::Install("文件名无效".into()))?;
    let dir = content_dir(game_dir, kind);
    fs::create_dir_all(&dir).map_err(|e| crate::error::io(&dir, e))?;
    let dest = dir.join(&file);
    if contains(game_dir, kind, &file) {
        return Err(Error::Install(format!("{file} 已经装过了")));
    }
    fs::copy(source, &dest).map_err(|e| crate::error::io(&dest, e))?;
    let mut index = load(game_dir);
    let meta = inspect_archive(&dest, kind, &file);
    let entry = InstalledEntry {
        kind,
        file,
        enabled: true,
        name: meta.name,
        mod_id: meta.mod_id,
        source: None,
        project_id: None,
        version_id: None,
        version_number: None,
        size: None,
        modified: None,
    };
    index.entries.push(entry.clone());
    save(game_dir, &index)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "nailong-installed-{tag}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(dir.join("mods")).unwrap();
        dir
    }

    fn write_mod(path: &Path, id: &str, name: &str) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(format!(r#"{{"id":"{id}","name":"{name}","version":"1.0"}}"#).as_bytes())
            .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn rescans_only_files_that_changed() {
        let dir = temp_dir("stamp");
        let jar = dir.join("mods/cool.jar");
        write_mod(&jar, "cool", "Cool");
        let first = scan(&dir, ContentKind::Mod).unwrap();
        assert_eq!(first[0].name, "Cool");
        assert!(first[0].size.is_some() && first[0].modified.is_some());
        // 改掉索引里的名字：文件没变时沿用索引，说明没有重新打开压缩包。
        let mut index = load(&dir);
        index.entries[0].name = "Cached".into();
        save(&dir, &index).unwrap();
        assert_eq!(scan(&dir, ContentKind::Mod).unwrap()[0].name, "Cached");
        // 禁用只是改名，大小和时间不变，状态要跟上。
        fs::rename(&jar, dir.join("mods/cool.jar.disabled")).unwrap();
        let disabled = scan(&dir, ContentKind::Mod).unwrap();
        assert!(!disabled[0].enabled);
        // 文件换了：重新读出元数据里的 ID。
        fs::rename(dir.join("mods/cool.jar.disabled"), &jar).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_mod(&jar, "other", "Other Mod With Longer Name");
        let changed = scan(&dir, ContentKind::Mod).unwrap();
        assert_eq!(changed[0].mod_id.as_deref(), Some("other"));
        // 外面删掉的文件从列表里消失。
        fs::remove_file(&jar).unwrap();
        assert!(scan(&dir, ContentKind::Mod).unwrap().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    /// 150 个、每个 300 个条目的 jar：第一次要打开每个压缩包，之后只看大小和修改时间。
    #[test]
    #[ignore = "计时用，手动跑"]
    fn scan_speed_with_many_mods() {
        let dir = temp_dir("speed");
        for i in 0..150 {
            let file = fs::File::create(dir.join(format!("mods/mod-{i}.jar"))).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            for j in 0..300 {
                zip.start_file(
                    format!("assets/m{i}/f{j}.json"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                zip.write_all(&[b'x'; 512]).unwrap();
            }
            zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(format!(r#"{{"id":"m{i}","name":"Mod {i}"}}"#).as_bytes())
                .unwrap();
            zip.finish().unwrap();
        }
        let started = std::time::Instant::now();
        assert_eq!(scan_all(&dir).unwrap().entries.len(), 150);
        let first = started.elapsed();
        let started = std::time::Instant::now();
        assert_eq!(scan_all(&dir).unwrap().entries.len(), 150);
        let second = started.elapsed();
        println!("first scan {first:?}, cached scan {second:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn classifies_dropped_files() {
        let dir = temp_dir("classify");
        let zip_with = |name: &str, entry: &str| {
            let path = dir.join(name);
            let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
            zip.start_file(entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"{}").unwrap();
            zip.finish().unwrap();
            path
        };
        assert_eq!(
            classify(&zip_with("a.zip", "pack.mcmeta")),
            Dropped::Content(ContentKind::ResourcePack)
        );
        assert_eq!(
            classify(&zip_with("b.zip", "shaders/final.fsh")),
            Dropped::Content(ContentKind::Shader)
        );
        assert_eq!(
            classify(&zip_with("c.zip", "manifest.json")),
            Dropped::Modpack
        );
        assert_eq!(classify(&zip_with("d.zip", "readme.txt")), Dropped::Archive);
        assert_eq!(
            classify(&dir.join("x.jar")),
            Dropped::Content(ContentKind::Mod)
        );
        assert_eq!(classify(&dir.join("x.mrpack")), Dropped::Modpack);
        assert_eq!(classify(&dir.join("notes.txt")), Dropped::Unknown);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn import_refuses_a_file_that_is_installed_but_disabled() {
        let dir = temp_dir("import-disabled");
        let source = dir.join("cool.jar");
        write_mod(&source, "cool", "Cool");
        fs::write(dir.join("mods/cool.jar.disabled"), b"x").unwrap();
        assert!(contains(&dir, ContentKind::Mod, "cool.jar"));
        assert!(import_local(&dir, ContentKind::Mod, &source).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scans_and_toggles_mods() {
        let dir = temp_dir("toggle");
        let path = dir.join("mods/example.jar");
        write_mod(&path, "example", "Example Mod");
        let entries = scan(&dir, ContentKind::Mod).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mod_id.as_deref(), Some("example"));
        let mut entry = entries[0].clone();
        set_enabled(&dir, &mut entry, false).unwrap();
        assert!(!entry.enabled);
        assert!(dir.join("mods/example.jar.disabled").exists());
        set_enabled(&dir, &mut entry, true).unwrap();
        assert!(dir.join("mods/example.jar").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_a_local_mod() {
        let dir = temp_dir("import");
        let source = dir.join("source.jar");
        write_mod(&source, "local", "Local Mod");
        let entry = import_local(&dir, ContentKind::Mod, &source).unwrap();
        assert_eq!(entry.mod_id.as_deref(), Some("local"));
        assert!(dir.join("mods/source.jar").exists());
        let entries = scan(&dir, ContentKind::Mod).unwrap();
        assert_eq!(entries.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_duplicate_project_files() {
        let entry = |file: &str| InstalledEntry {
            kind: ContentKind::Mod,
            file: file.to_string(),
            enabled: true,
            name: file.to_string(),
            mod_id: None,
            source: Some(Source::Modrinth),
            project_id: Some("P".into()),
            version_id: Some(file.into()),
            version_number: None,
            size: None,
            modified: None,
        };
        let groups = duplicates(&[entry("a.jar"), entry("b.jar")]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.len(), 2);
    }

    #[test]
    fn rejects_index_entries_that_escape_the_game_dir() {
        let dir = temp_dir("traversal");
        let outside = dir.parent().unwrap().join(format!(
            "nailong-victim-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&outside, b"keep").unwrap();

        let crafted = InstalledEntry {
            kind: ContentKind::Mod,
            file: format!("../{}", outside.file_name().unwrap().to_string_lossy()),
            enabled: true,
            name: "evil".into(),
            mod_id: None,
            source: None,
            project_id: None,
            version_id: None,
            version_number: None,
            size: None,
            modified: None,
        };
        save(
            &dir,
            &ContentIndex {
                entries: vec![crafted.clone()],
            },
        )
        .unwrap();

        // 被篡改的索引在加载时就被丢弃。
        assert!(load(&dir).entries.is_empty());
        // 直接传进来的条目也不能绕过检查。
        assert!(remove(&dir, &crafted).is_err());
        assert!(set_enabled(&dir, &mut crafted.clone(), false).is_err());
        assert!(outside.exists());

        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&dir);
    }
}
