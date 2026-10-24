//! Git-style world snapshots: content-defined chunks, a shared object store
//! and per-world manifests. Backing up twice only stores changed chunks.

use crate::error::{Error, Result};
use crate::paths::Root;
use crate::saves;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MIN_CHUNK: usize = 64 * 1024;
const MAX_CHUNK: usize = 4 * 1024 * 1024;
const MASK: u64 = (1 << 18) - 1;
const WINDOW: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub interval_minutes: u64,
    /// 每个世界保留多少份快照。
    pub keep: usize,
    pub last_run: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 30,
            keep: 20,
            last_run: 0,
        }
    }
}

pub fn load_config(root: &Root) -> Config {
    let path = root.dir().join("backups.json");
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_config(root: &Root, config: &Config) -> Result<()> {
    let path = root.dir().join("backups.json");
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| Error::Install(format!("备份配置序列化失败: {e}")))?;
    crate::paths::write_atomic(&path, &bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub world: String,
    pub id: String,
    pub created_at: u64,
    pub bytes: u64,
    pub files: Vec<FileEntry>,
}

fn safe_name(name: &str) -> Result<&str> {
    let path = Path::new(name);
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(name),
        _ => Err(Error::Install(format!("非法的存档名: {name}"))),
    }
}

fn repo_dir(game_dir: &Path) -> PathBuf {
    game_dir.join("backups").join("repo")
}

fn objects_dir(game_dir: &Path) -> PathBuf {
    repo_dir(game_dir).join("objects")
}

fn snapshots_dir(game_dir: &Path, world: &str) -> PathBuf {
    repo_dir(game_dir).join("snapshots").join(world)
}

pub fn backup(game_dir: &Path, world: &str) -> Result<Snapshot> {
    let world = safe_name(world)?;
    let source = saves::saves_dir(game_dir).join(world);
    if !source.join("level.dat").is_file() {
        return Err(Error::Install(format!("找不到存档: {world}")));
    }
    let objects = objects_dir(game_dir);
    fs::create_dir_all(&objects).map_err(|e| crate::error::io(&objects, e))?;
    let mut files = Vec::new();
    let mut bytes = 0u64;
    walk(&source, Path::new(""), &mut |relative, path| {
        let mut file = fs::File::open(path).map_err(|e| crate::error::io(path, e))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| crate::error::io(path, e))?;
        let mut chunks = Vec::new();
        for chunk in split_chunks(&data) {
            chunks.push(store_chunk(&objects, chunk)?);
        }
        bytes += data.len() as u64;
        files.push(FileEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            size: data.len() as u64,
            chunks,
        });
        Ok(())
    })?;
    let created_at = now();
    let snapshot = Snapshot {
        world: world.to_string(),
        id: format!("{}-{}", now_millis(), next_seq()),
        created_at,
        bytes,
        files,
    };
    let dir = snapshots_dir(game_dir, world);
    fs::create_dir_all(&dir).map_err(|e| crate::error::io(&dir, e))?;
    let path = dir.join(format!("{}.json", snapshot.id));
    let data = serde_json::to_vec(&snapshot)
        .map_err(|e| Error::Install(format!("快照序列化失败: {e}")))?;
    crate::paths::write_atomic(&path, &data)?;
    Ok(snapshot)
}

pub fn list(game_dir: &Path, world: &str) -> Result<Vec<Snapshot>> {
    let world = safe_name(world)?;
    let dir = snapshots_dir(game_dir, world);
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
    // created_at 只到秒；同一秒内的多次备份用 id（毫秒-序号）兜底，
    // 否则 read_dir 的顺序会让“最新快照”不稳定，prune 可能删错。
    out.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
    Ok(out)
}

/// 恢复到 `saves/<world>`；目录已存在时改名为 `<world>-restored-<时间>`。
pub fn restore(game_dir: &Path, world: &str, id: &str) -> Result<PathBuf> {
    let world = safe_name(world)?;
    let path = snapshots_dir(game_dir, world).join(format!("{id}.json"));
    let data = fs::read(&path).map_err(|e| crate::error::io(&path, e))?;
    let snapshot: Snapshot =
        serde_json::from_slice(&data).map_err(|e| Error::Install(format!("快照损坏: {e}")))?;
    let saves = saves::saves_dir(game_dir);
    fs::create_dir_all(&saves).map_err(|e| crate::error::io(&saves, e))?;
    let mut target = saves.join(world);
    if target.exists() {
        target = saves.join(format!("{world}-restored-{}", now()));
    }
    let objects = objects_dir(game_dir);
    for file in &snapshot.files {
        let relative = safe_relative(&file.path)?;
        let dest = target.join(&relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
        }
        let mut bytes = Vec::with_capacity(file.size as usize);
        for hash in &file.chunks {
            let chunk = read_chunk(&objects, hash)?;
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() as u64 != file.size {
            return Err(Error::Install(format!("快照文件大小不符: {}", file.path)));
        }
        fs::write(&dest, &bytes).map_err(|e| crate::error::io(&dest, e))?;
    }
    Ok(target)
}

/// 对象库的块数和占用字节数（用于界面展示“仓库”规模）。
pub fn repo_stats(game_dir: &Path) -> (usize, u64) {
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

pub fn delete(game_dir: &Path, world: &str, id: &str) -> Result<()> {
    let world = safe_name(world)?;
    let path = snapshots_dir(game_dir, world).join(format!("{id}.json"));
    fs::remove_file(&path).map_err(|e| crate::error::io(&path, e))
}

/// 每个世界只保留最新 N 份快照，并清掉没有任何快照引用的对象。
pub fn prune(game_dir: &Path, keep: usize) -> Result<(usize, usize)> {
    let mut removed = 0usize;
    let mut referenced = BTreeSet::new();
    let worlds = saves::worlds(game_dir)?;
    for world in &worlds {
        let items = list(game_dir, &world.name)?;
        for (index, snapshot) in items.iter().enumerate() {
            if index >= keep {
                let path =
                    snapshots_dir(game_dir, &world.name).join(format!("{}.json", snapshot.id));
                let _ = fs::remove_file(path);
                removed += 1;
                continue;
            }
            for file in &snapshot.files {
                for chunk in &file.chunks {
                    referenced.insert(chunk.clone());
                }
            }
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
                    if !referenced.contains(&full) {
                        let _ = fs::remove_file(entry.path());
                        deleted += 1;
                    }
                }
            }
        }
    }
    Ok((removed, deleted))
}

fn safe_relative(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return Err(Error::Install(format!("快照里有越界路径: {path}"))),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(Error::Install("快照里有空路径".into()));
    }
    Ok(out)
}

fn walk(
    root: &Path,
    relative: &Path,
    handle: &mut impl FnMut(&Path, &Path) -> Result<()>,
) -> Result<()> {
    let entries = fs::read_dir(root).map_err(|e| crate::error::io(root, e))?;
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|entry| entry.file_name());
    for entry in items {
        let path = entry.path();
        let inner = relative.join(entry.file_name());
        let meta = entry.metadata().map_err(|e| crate::error::io(&path, e))?;
        if meta.is_dir() {
            walk(&path, &inner, handle)?;
        } else if meta.is_file() && meta.len() <= 512 * 1024 * 1024 {
            handle(&inner, &path)?;
        }
    }
    Ok(())
}

fn store_chunk(objects: &Path, data: &[u8]) -> Result<String> {
    let hash = crate::http::hex(&Sha1::digest(data));
    let dir = objects.join(&hash[..2]);
    let path = dir.join(&hash[2..]);
    if !path.exists() {
        fs::create_dir_all(&dir).map_err(|e| crate::error::io(&dir, e))?;
        crate::paths::write_atomic(&path, data)?;
    }
    Ok(hash)
}

fn read_chunk(objects: &Path, hash: &str) -> Result<Vec<u8>> {
    if hash.len() < 3 {
        return Err(Error::Install("快照里的块哈希无效".into()));
    }
    let path = objects.join(&hash[..2]).join(&hash[2..]);
    fs::read(&path).map_err(|e| crate::error::io(&path, e))
}

fn split_chunks(data: &[u8]) -> Vec<&[u8]> {
    let gear = gear_table();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut hash: u64 = 0;
    let mut index = 0usize;
    while index < data.len() {
        hash = hash.rotate_left(1).wrapping_add(gear[data[index] as usize]);
        if index >= WINDOW {
            hash ^= gear[data[index - WINDOW] as usize].rotate_left(WINDOW as u32);
        }
        let len = index - start + 1;
        if len >= MAX_CHUNK || (len >= MIN_CHUNK && (hash & MASK) == 0) {
            out.push(&data[start..=index]);
            start = index + 1;
            hash = 0;
        }
        index += 1;
    }
    if start < data.len() {
        out.push(&data[start..]);
    }
    out
}

fn gear_table() -> &'static [u64; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u64; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut table = [0u64; 256];
        for slot in &mut table {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *slot = state;
        }
        table
    })
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn next_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nailong-snap-{tag}-{}-{}",
            std::process::id(),
            now()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_world(game: &Path, name: &str, payload: &[u8]) {
        let dir = saves::saves_dir(game).join(name).join("region");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            saves::saves_dir(game).join(name).join("level.dat"),
            b"level",
        )
        .unwrap();
        fs::write(dir.join("r.0.0.mca"), payload).unwrap();
    }

    #[test]
    fn chunks_are_content_defined_and_shared() {
        let mut first = vec![7u8; 900_000];
        let chunks_a = split_chunks(&first);
        assert!(chunks_a.len() > 1);
        let set_a: BTreeSet<Vec<u8>> = chunks_a.iter().map(|c| c.to_vec()).collect();
        // 中间改一小段，大部分块应该保持不变（内容定义分块）。
        first[400_000..400_100].fill(9);
        let chunks_b = split_chunks(&first);
        let shared = chunks_b
            .iter()
            .filter(|c| set_a.iter().any(|x| x.as_slice() == **c))
            .count();
        assert!(
            shared + 2 >= chunks_b.len(),
            "共享块太少: {shared}/{}",
            chunks_b.len()
        );
    }

    #[test]
    fn snapshots_dedupe_and_restore() {
        let game = temp_dir("roundtrip");
        let mut payload = vec![1u8; 700_000];
        write_world(&game, "World", &payload);
        let first = backup(&game, "World").unwrap();
        let objects_after_first = count_objects(&game);
        payload[300_000..300_050].fill(2);
        write_world(&game, "World", &payload);
        let second = backup(&game, "World").unwrap();
        let objects_after_second = count_objects(&game);
        assert!(second.id != first.id);
        assert!(
            objects_after_second < objects_after_first * 2,
            "没有去重: {objects_after_first} -> {objects_after_second}"
        );
        let items = list(&game, "World").unwrap();
        assert_eq!(items.len(), 2);

        fs::remove_dir_all(saves::saves_dir(&game).join("World")).unwrap();
        let target = restore(&game, "World", &second.id).unwrap();
        assert!(target.join("level.dat").is_file());
        let restored = fs::read(target.join("region/r.0.0.mca")).unwrap();
        assert_eq!(restored.len(), payload.len());
        assert_eq!(restored[300_000], 2);
        assert_eq!(restored[500_000], 1);
        let _ = fs::remove_dir_all(&game);
    }

    #[test]
    fn prune_keeps_latest_and_collects_garbage() {
        let game = temp_dir("prune");
        let mut payload = vec![3u8; 600_000];
        write_world(&game, "World", &payload);
        let mut ids = Vec::new();
        for round in 0..4 {
            payload[100_000 + round] = round as u8;
            write_world(&game, "World", &payload);
            ids.push(backup(&game, "World").unwrap().id);
        }
        assert_eq!(list(&game, "World").unwrap().len(), 4);
        let (removed, _) = prune(&game, 2).unwrap();
        assert_eq!(removed, 2);
        let remaining = list(&game, "World").unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].id, ids[3]);
        let _ = fs::remove_dir_all(&game);
    }

    fn count_objects(game: &Path) -> usize {
        let root = objects_dir(game);
        let mut count = 0;
        if let Ok(prefixes) = fs::read_dir(&root) {
            for prefix in prefixes.flatten() {
                if let Ok(entries) = fs::read_dir(prefix.path()) {
                    count += entries.count();
                }
            }
        }
        count
    }
}
