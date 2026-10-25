//! Per-instance content rollback points.
//!
//! Mods, configs, resource packs and the content index are stored as whole-file
//! objects keyed by SHA-1. Unchanged jars are reused across rollback points, so
//! taking a point before every install is cheap. World saves have their own
//! chunk-based snapshots and are deliberately outside this module.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ROOTS: [&str; 4] = ["mods", "config", "resourcepacks", "shaderpacks"];
const ROOT_FILES: [&str; 2] = [".nailong-content.json", "options.txt"];
/// How many rollback points to keep per instance.
pub const KEEP: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub created_at: u64,
    pub reason: String,
    pub bytes: u64,
    pub files: Vec<Entry>,
    /// Loader metadata/binary files managed by this point (direct children of
    /// `game_dir`). Older snapshots lack this field and default to empty.
    #[serde(default)]
    pub extra_files: Vec<String>,
}

fn instance_dir(game_dir: &Path) -> PathBuf {
    game_dir.join("backups").join("instance")
}

fn objects_dir(game_dir: &Path) -> PathBuf {
    instance_dir(game_dir).join("objects")
}

fn snapshots_dir(game_dir: &Path) -> PathBuf {
    instance_dir(game_dir).join("snapshots")
}

fn safe_relative(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return Err(Error::Install(format!("恢复点里有越界路径: {path}"))),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(Error::Install("恢复点里有空路径".into()));
    }
    Ok(out)
}

fn object_path(game_dir: &Path, hash: &str) -> Result<PathBuf> {
    if hash.len() < 3 {
        return Err(Error::Install("恢复点里的文件哈希无效".into()));
    }
    Ok(objects_dir(game_dir).join(&hash[..2]).join(&hash[2..]))
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(path).map_err(|e| crate::error::io(path, e))?;
    let mut sha = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| crate::error::io(path, e))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        sha.update(&buffer[..read]);
    }
    Ok((crate::http::hex(&sha.finalize()), size))
}

fn store_object(game_dir: &Path, source: &Path, hash: &str) -> Result<()> {
    let dest = object_path(game_dir, hash)?;
    if dest.is_file() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
    }
    let temp = instance_dir(game_dir).join(format!(".tmp-{}-{}", std::process::id(), next_seq()));
    fs::copy(source, &temp).map_err(|e| crate::error::io(&temp, e))?;
    fs::rename(&temp, &dest).map_err(|e| {
        let _ = fs::remove_file(&temp);
        crate::error::io(&dest, e)
    })?;
    Ok(())
}

fn walk(
    root: &Path,
    relative: &Path,
    add: &mut impl FnMut(&Path, &Path) -> Result<()>,
) -> Result<()> {
    let entries = fs::read_dir(root).map_err(|e| crate::error::io(root, e))?;
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|entry| entry.file_name());
    for entry in items {
        let file_type = entry
            .file_type()
            .map_err(|e| crate::error::io(entry.path(), e))?;
        let path = entry.path();
        let inner = relative.join(entry.file_name());
        if file_type.is_dir() {
            walk(&path, &inner, add)?;
        } else if file_type.is_file() {
            add(&inner, &path)?;
        }
    }
    Ok(())
}

/// Take a rollback point of the current content state.
///
/// The manifest is written after every object is safely in the store; old
/// points are pruned to [`KEEP`].
pub fn backup(game_dir: &Path, reason: &str) -> Result<Snapshot> {
    backup_extra(game_dir, reason, &[])
}

/// Like [`backup`], but also snapshots direct-child files such as the version
/// JSON and patched client jar. Files absent at backup time are recorded so a
/// later restore removes a loader file that did not exist before.
pub fn backup_extra(game_dir: &Path, reason: &str, extra: &[String]) -> Result<Snapshot> {
    let objects = objects_dir(game_dir);
    fs::create_dir_all(&objects).map_err(|e| crate::error::io(&objects, e))?;
    let mut files: Vec<Entry> = Vec::new();
    let mut bytes = 0u64;
    let mut add = |relative: &Path, path: &Path| -> Result<()> {
        let relative = relative.to_string_lossy().replace('\\', "/");
        if files.iter().any(|entry| entry.path == relative) {
            return Ok(());
        }
        let (hash, size) = hash_file(path)?;
        store_object(game_dir, path, &hash)?;
        bytes += size;
        files.push(Entry {
            path: relative,
            size,
            hash,
        });
        Ok(())
    };
    for root in ROOTS {
        let base = game_dir.join(root);
        if base.is_dir() {
            walk(&base, Path::new(root), &mut add)?;
        }
    }
    for name in ROOT_FILES {
        let path = game_dir.join(name);
        if path.is_file() {
            add(Path::new(name), &path)?;
        }
    }
    let mut extra_files = Vec::new();
    for name in extra {
        let relative = safe_relative(name)?;
        if relative.components().count() != 1 {
            return Err(Error::Install(format!(
                "恢复点附加文件必须是直接子文件: {name}"
            )));
        }
        if extra_files.contains(name) {
            continue;
        }
        let path = game_dir.join(&relative);
        if path.is_file() {
            add(&relative, &path)?;
        }
        extra_files.push(name.clone());
    }
    extra_files.sort();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let created_at = now();
    let snapshot = Snapshot {
        id: format!("{}-{:06}", now_millis(), next_seq()),
        created_at,
        reason: if reason.trim().is_empty() {
            "手动恢复点".to_string()
        } else {
            reason.trim().to_string()
        },
        bytes,
        files,
        extra_files,
    };
    let dir = snapshots_dir(game_dir);
    fs::create_dir_all(&dir).map_err(|e| crate::error::io(&dir, e))?;
    let path = dir.join(format!("{}.json", snapshot.id));
    let data = serde_json::to_vec(&snapshot)
        .map_err(|e| Error::Install(format!("恢复点序列化失败: {e}")))?;
    crate::paths::write_atomic(&path, &data)?;
    let _ = prune(game_dir, KEEP);
    Ok(snapshot)
}

pub fn list(game_dir: &Path) -> Result<Vec<Snapshot>> {
    let dir = snapshots_dir(game_dir);
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        if let Ok(snapshot) = serde_json::from_slice::<Snapshot>(&data) {
            out.push(snapshot);
        }
    }
    out.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
    Ok(out)
}

/// Restore a point. A fresh safety point is taken first, then the managed
/// content roots are replaced. Worlds, logs and the object store are untouched.
pub fn restore(game_dir: &Path, id: &str) -> Result<usize> {
    let path = snapshots_dir(game_dir).join(format!("{id}.json"));
    let data = fs::read(&path).map_err(|e| crate::error::io(&path, e))?;
    let snapshot: Snapshot =
        serde_json::from_slice(&data).map_err(|e| Error::Install(format!("恢复点损坏: {e}")))?;
    backup(game_dir, "恢复前自动留档")?;
    for root in ROOTS {
        let base = game_dir.join(root);
        if base.is_dir() {
            fs::remove_dir_all(&base).map_err(|e| crate::error::io(&base, e))?;
        }
    }
    for name in ROOT_FILES {
        let path = game_dir.join(name);
        if path.is_file() {
            fs::remove_file(&path).map_err(|e| crate::error::io(&path, e))?;
        }
    }
    // Loader files are a small, explicit set; remove them so restoring a
    // pre-loader state does not leave a stale patched jar behind.
    for name in &snapshot.extra_files {
        let relative = safe_relative(name)?;
        if relative.components().count() != 1 {
            return Err(Error::Install(format!(
                "恢复点附加文件必须是直接子文件: {name}"
            )));
        }
        let path = game_dir.join(&relative);
        if path.is_file() {
            fs::remove_file(&path).map_err(|e| crate::error::io(&path, e))?;
        }
    }
    for file in &snapshot.files {
        let relative = safe_relative(&file.path)?;
        let dest = game_dir.join(&relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
        }
        let source = object_path(game_dir, &file.hash)?;
        let (hash, size) = hash_file(&source)?;
        if hash != file.hash || size != file.size {
            return Err(Error::Install(format!("恢复点对象校验失败: {}", file.path)));
        }
        fs::copy(&source, &dest).map_err(|e| crate::error::io(&dest, e))?;
    }
    Ok(snapshot.files.len())
}

pub fn delete(game_dir: &Path, id: &str) -> Result<()> {
    let path = snapshots_dir(game_dir).join(format!("{id}.json"));
    fs::remove_file(&path).map_err(|e| crate::error::io(&path, e))?;
    let _ = prune(game_dir, KEEP);
    Ok(())
}

/// Keep the newest `keep` manifests and GC every object they do not reference.
pub fn prune(game_dir: &Path, keep: usize) -> Result<(usize, usize)> {
    let items = list(game_dir)?;
    let mut referenced = std::collections::BTreeSet::new();
    let mut removed = 0usize;
    for (index, snapshot) in items.iter().enumerate() {
        if index >= keep {
            let path = snapshots_dir(game_dir).join(format!("{}.json", snapshot.id));
            match fs::remove_file(path) {
                Ok(()) => removed += 1,
                Err(_) => {
                    // Keep objects for a manifest we failed to remove; otherwise
                    // the point would stay visible but unreadable.
                    for file in &snapshot.files {
                        referenced.insert(file.hash.clone());
                    }
                }
            }
            continue;
        }
        for file in &snapshot.files {
            referenced.insert(file.hash.clone());
        }
    }
    let objects = objects_dir(game_dir);
    let mut deleted = 0usize;
    if let Ok(prefixes) = fs::read_dir(&objects) {
        for prefix in prefixes.flatten() {
            let dir = prefix.path();
            if !dir.is_dir() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let full = format!("{}{}", prefix.file_name().to_string_lossy(), name);
                    if !referenced.contains(&full) && fs::remove_file(entry.path()).is_ok() {
                        deleted += 1;
                    }
                }
            }
        }
    }
    Ok((removed, deleted))
}

/// Object count and bytes used by the instance object store.
pub fn stats(game_dir: &Path) -> (usize, u64) {
    let objects = objects_dir(game_dir);
    let mut count = 0usize;
    let mut bytes = 0u64;
    if let Ok(prefixes) = fs::read_dir(&objects) {
        for prefix in prefixes.flatten() {
            if let Ok(entries) = fs::read_dir(prefix.path()) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata()
                        && meta.is_file()
                    {
                        count += 1;
                        bytes += meta.len();
                    }
                }
            }
        }
    }
    (count, bytes)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_game(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nailong-rollback-{tag}-{}-{}",
            std::process::id(),
            now_millis()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn backup_and_restore_removes_added_mods() {
        let game = temp_game("restore");
        fs::create_dir_all(game.join("mods")).unwrap();
        fs::create_dir_all(game.join("config")).unwrap();
        fs::write(game.join("mods/a.jar"), b"mod-a").unwrap();
        fs::write(game.join("config/options.txt"), b"v1").unwrap();
        fs::create_dir_all(game.join("saves/world")).unwrap();
        fs::write(game.join("saves/world/level.dat"), b"world").unwrap();
        let point = backup(&game, "测试").unwrap();
        assert_eq!(point.files.len(), 2);

        fs::write(game.join("mods/b.jar"), b"bad-mod").unwrap();
        fs::write(game.join("config/options.txt"), b"broken").unwrap();
        let count = restore(&game, &point.id).unwrap();
        assert_eq!(count, 2);
        assert_eq!(fs::read(game.join("mods/a.jar")).unwrap(), b"mod-a");
        assert!(!game.join("mods/b.jar").exists());
        assert_eq!(fs::read(game.join("config/options.txt")).unwrap(), b"v1");
        assert_eq!(
            fs::read(game.join("saves/world/level.dat")).unwrap(),
            b"world"
        );
        // A safety point for the broken state must exist.
        let points = list(&game).unwrap();
        assert!(points.iter().any(|point| point.reason.contains("恢复")));
    }

    #[test]
    fn loader_files_restore_exactly() {
        let game = temp_game("loader");
        fs::write(game.join("1.21.1.json"), b"loader-v1").unwrap();
        fs::write(game.join("1.21.1.jar"), b"jar-v1").unwrap();
        let extra = vec![
            "1.21.1.json".to_string(),
            "1.21.1.jar".to_string(),
            "playerdata.json".to_string(),
        ];
        let point = backup_extra(&game, "加载器更新前", &extra).unwrap();
        assert_eq!(point.files.len(), 2);
        assert_eq!(point.extra_files.len(), 3);

        fs::write(game.join("1.21.1.json"), b"loader-v2").unwrap();
        fs::remove_file(game.join("1.21.1.jar")).unwrap();
        fs::write(game.join("playerdata.json"), b"added-later").unwrap();
        restore(&game, &point.id).unwrap();
        assert_eq!(fs::read(game.join("1.21.1.json")).unwrap(), b"loader-v1");
        assert_eq!(fs::read(game.join("1.21.1.jar")).unwrap(), b"jar-v1");
        assert!(!game.join("playerdata.json").exists());
    }

    #[test]
    fn old_snapshot_without_extra_files_still_loads() {
        let json = r#"{"id":"1","created_at":0,"reason":"old","bytes":0,"files":[]}"#;
        let snapshot: Snapshot = serde_json::from_str(json).unwrap();
        assert!(snapshot.extra_files.is_empty());
    }

    #[test]
    fn unchanged_files_share_objects() {
        let game = temp_game("share");
        fs::create_dir_all(game.join("mods")).unwrap();
        fs::write(game.join("mods/a.jar"), vec![7u8; 300_000]).unwrap();
        backup(&game, "one").unwrap();
        let (first_count, _) = stats(&game);
        fs::create_dir_all(game.join("config")).unwrap();
        fs::write(game.join("config/x.toml"), b"config").unwrap();
        backup(&game, "two").unwrap();
        let (second_count, _) = stats(&game);
        assert_eq!(first_count, 1);
        assert_eq!(second_count, 2);
    }

    #[test]
    fn prune_keeps_latest_and_collects_garbage() {
        let game = temp_game("prune");
        fs::create_dir_all(game.join("mods")).unwrap();
        for index in 0..4 {
            fs::write(game.join("mods").join(format!("{index}.jar")), [index]).unwrap();
            backup(&game, "point").unwrap();
        }
        let (removed, _) = prune(&game, 2).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(list(&game).unwrap().len(), 2);
        // The two newest points are cumulative, so they still reference all
        // four distinct one-byte mods.
        let (objects, _) = stats(&game);
        assert_eq!(objects, 4);
    }

    #[test]
    fn rejects_traversal_in_manifest() {
        assert!(safe_relative("../outside").is_err());
        assert!(safe_relative("mods/a.jar").is_ok());
    }
}
