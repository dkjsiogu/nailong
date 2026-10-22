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

fn actual_path(game_dir: &Path, entry: &InstalledEntry) -> PathBuf {
    let suffix = if entry.enabled { "" } else { DISABLED_SUFFIX };
    content_dir(game_dir, entry.kind).join(format!("{}{suffix}", entry.file))
}

pub fn load(game_dir: &Path) -> ContentIndex {
    fs::read(index_path(game_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
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
            if !path.is_file() {
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
            let meta = inspect_archive(&path, kind, &file);
            let existing = index
                .entries
                .iter()
                .find(|entry| entry.kind == kind && entry.file == file);
            let entry = InstalledEntry {
                kind,
                file: file.clone(),
                enabled,
                name: existing
                    .map(|entry| entry.name.clone())
                    .unwrap_or(meta.name),
                mod_id: existing
                    .and_then(|entry| entry.mod_id.clone())
                    .or(meta.mod_id),
                source: existing.and_then(|entry| entry.source),
                project_id: existing.and_then(|entry| entry.project_id.clone()),
                version_id: existing.and_then(|entry| entry.version_id.clone()),
                version_number: existing.and_then(|entry| entry.version_number.clone()),
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

pub fn import_local(game_dir: &Path, kind: ContentKind, source: &Path) -> Result<InstalledEntry> {
    let file = source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| Error::Install("文件名无效".into()))?;
    let dir = content_dir(game_dir, kind);
    fs::create_dir_all(&dir).map_err(|e| crate::error::io(&dir, e))?;
    let dest = dir.join(&file);
    if dest.exists() {
        return Err(Error::Install(format!("{} 已存在", dest.display())));
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
        };
        let groups = duplicates(&[entry("a.jar"), entry("b.jar")]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1.len(), 2);
    }
}
