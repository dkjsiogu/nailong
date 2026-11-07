//! 截图的「时间、地点、人物」。
//!
//! - **时间**：游戏按本地时间给截图起名（`2026-11-07_21.34.12.png`），直接从文件名读；
//!   名字不合这个规矩时用修改时间。
//! - **地点**：哪个版本、哪个存档或服务器。**人物**：用哪个账户截的。
//! - 从 Cairn 启动的游戏：截图那一刻（日志里出现保存截图的那行）就记下来，写进 PNG 自带的
//!   文字块（iTXt，关键字 `Cairn`），像照片的 EXIF 一样跟着图片走——日志删了、图片挪走了都还在。
//!   写入是原子的，不动像素，保留原来的修改时间。设置里可以关掉，关掉后记在数据目录的 `shots.json`。
//! - 以前的截图：翻游戏日志推断，结果也记在 `shots.json`，之后日志删了也不丢；认不出来的就是「不知道」。

use crate::error::{Error, Result};
use crate::paths::Root;
use chrono::{DateTime, Local, NaiveDateTime};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// 一张截图知道的事。全部可选：不知道就是 None。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// 给人看的版本名，例如「Fabric 1.21.1」或整合包的名字。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 单人世界：`saves/` 下的文件夹名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    /// 多人游戏：服务器地址（默认端口不写）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_uuid: Option<String>,
    /// 账户类型：「正版」「LittleSkin」「离线」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// 怎么知道的：`live`（截图时记下）、`log`（从日志推断）、`manual`（玩家指定）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Meta {
    /// 拿另一份补上自己不知道的字段。
    pub fn fill_from(&mut self, other: &Meta) {
        let fill = |mine: &mut Option<String>, theirs: &Option<String>| {
            if mine.is_none() {
                mine.clone_from(theirs);
            }
        };
        fill(&mut self.version, &other.version);
        if self.world.is_none() && self.server.is_none() {
            self.world.clone_from(&other.world);
            self.server.clone_from(&other.server);
        }
        fill(&mut self.player, &other.player);
        fill(&mut self.player_uuid, &other.player_uuid);
        fill(&mut self.account, &other.account);
        fill(&mut self.source, &other.source);
    }

    pub fn is_empty(&self) -> bool {
        self == &Meta::default()
    }
}

/// 启动游戏时就知道的部分：版本和账户。截图时再补上存档或服务器。
#[derive(Debug, Clone)]
pub struct Tag {
    pub version: String,
    pub player: String,
    pub player_uuid: String,
    pub account: String,
    /// 写进图片本身；false 时只记在 `shots.json`。
    pub embed: bool,
}

// ---------------------------------------------------------------- PNG 文字块

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const KEYWORD: &str = "Cairn";

/// 读图片里 Cairn 写的信息。只读到第一个图像数据块为止，不读整张图。
pub fn read_meta(path: &Path) -> Option<Meta> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut signature = [0u8; 8];
    file.read_exact(&mut signature).ok()?;
    if signature != SIGNATURE {
        return None;
    }
    loop {
        let mut head = [0u8; 8];
        file.read_exact(&mut head).ok()?;
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let kind = &head[4..8];
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        if kind == b"iTXt" && len < 1 << 20 {
            let mut data = vec![0u8; len as usize];
            file.read_exact(&mut data).ok()?;
            if let Some(text) = itxt_text(&data, KEYWORD) {
                return serde_json::from_slice(text).ok();
            }
            file.seek(SeekFrom::Current(4)).ok()?;
        } else {
            file.seek(SeekFrom::Current(len as i64 + 4)).ok()?;
        }
    }
}

/// 把信息写进图片：放在 IHDR 后面，替换掉旧的那份。像素一个字节都不动，保留修改时间。
pub fn write_meta(path: &Path, meta: &Meta) -> Result<()> {
    let bytes = std::fs::read(path).map_err(|e| crate::error::io(path, e))?;
    let out = with_meta(&bytes, meta)
        .ok_or_else(|| Error::Install(format!("不是完整的 PNG: {}", path.display())))?;
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    crate::paths::write_atomic(path, &out)?;
    if let Some(time) = modified
        && let Ok(file) = std::fs::File::options().write(true).open(path)
    {
        let _ = file.set_modified(time);
    }
    Ok(())
}

fn with_meta(bytes: &[u8], meta: &Meta) -> Option<Vec<u8>> {
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return None;
    }
    let mut chunks = Vec::new();
    let mut at = 8;
    let mut ended = false;
    while at + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let end = at.checked_add(12 + len)?;
        if end > bytes.len() {
            return None;
        }
        let kind = &bytes[at + 4..at + 8];
        let data = &bytes[at + 8..at + 8 + len];
        let ours = kind == b"iTXt" && itxt_text(data, KEYWORD).is_some();
        if !ours {
            chunks.push(&bytes[at..end]);
        }
        at = end;
        if kind == b"IEND" {
            ended = true;
            break;
        }
    }
    if !ended || chunks.first().map(|c| &c[4..8]) != Some(b"IHDR") {
        return None;
    }
    let json = serde_json::to_vec(meta).ok()?;
    let mut data = Vec::with_capacity(json.len() + 16);
    data.extend_from_slice(KEYWORD.as_bytes());
    data.extend_from_slice(&[0, 0, 0, 0, 0]); // 结束符、不压缩、压缩方法、语言结束、译名结束
    data.extend_from_slice(&json);
    let mut out = Vec::with_capacity(bytes.len() + data.len() + 12);
    out.extend_from_slice(&SIGNATURE);
    out.extend_from_slice(chunks[0]);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"iTXt");
    crc.update(&data);
    out.extend_from_slice(b"iTXt");
    out.extend_from_slice(&data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
    for chunk in &chunks[1..] {
        out.extend_from_slice(chunk);
    }
    Some(out)
}

/// iTXt 块：关键字\0 压缩标志 压缩方法 语言\0 译名\0 正文。只认没压缩的。
fn itxt_text<'a>(data: &'a [u8], keyword: &str) -> Option<&'a [u8]> {
    let rest = data.strip_prefix(keyword.as_bytes())?.strip_prefix(&[0])?;
    let (&flag, rest) = rest.split_first()?;
    if flag != 0 {
        return None;
    }
    let rest = &rest[1..];
    let lang_end = rest.iter().position(|&b| b == 0)?;
    let rest = &rest[lang_end + 1..];
    let translated_end = rest.iter().position(|&b| b == 0)?;
    Some(&rest[translated_end + 1..])
}

// ---------------------------------------------------------------- 时间

/// 游戏起的文件名里的本地时间：`2026-11-07_21.34.12.png`、`2026-11-07_21.34.12_2.png`。
pub fn time_from_name(name: &str) -> Option<NaiveDateTime> {
    let stem = name.strip_suffix(".png")?;
    let base = stem.get(..19)?;
    let tail = &stem[19..];
    if !(tail.is_empty()
        || tail
            .strip_prefix('_')
            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()) && !n.is_empty()))
    {
        return None;
    }
    NaiveDateTime::parse_from_str(base, "%Y-%m-%d_%H.%M.%S").ok()
}

/// 本地时间。
pub fn local(time: SystemTime) -> NaiveDateTime {
    DateTime::<Local>::from(time).naive_local()
}

// ---------------------------------------------------------------- 日志

/// 日志里一行保存截图的消息，返回文件名。消息会按游戏语言翻译，所以不认文字，只认
/// 聊天行里那个按时间起的文件名。
pub fn saved_name(line: &str) -> Option<&str> {
    if !line.contains("[CHAT]") {
        return None;
    }
    let mut from = 0;
    while let Some(found) = line[from..].find(".png") {
        let end = from + found + 4;
        let start = line[..end - 4]
            .rfind(|c: char| !(c.is_ascii_digit() || c == '-' || c == '_' || c == '.'))
            .map(|i| i + line[i..].chars().next().map_or(1, char::len_utf8))
            .unwrap_or(0);
        let name = &line[start..end];
        if time_from_name(name).is_some() {
            return Some(name);
        }
        from = end;
    }
    None
}

/// 一边读日志一边记着「现在在哪」。
#[derive(Debug, Clone, Default)]
struct Context {
    server: Option<String>,
    level: Option<String>,
    singleplayer: bool,
    player: Option<String>,
}

impl Context {
    /// 喂一行日志；这行是保存截图的话返回文件名。
    fn feed(&mut self, line: &str) -> Option<String> {
        if let Some(rest) = after(line, "Connecting to ") {
            if let Some((host, port)) = rest.trim().rsplit_once(", ")
                && port.trim().parse::<u16>().is_ok()
            {
                let port = port.trim();
                self.server = Some(if port == "25565" {
                    host.trim().to_string()
                } else {
                    format!("{}:{port}", host.trim())
                });
                self.singleplayer = false;
                self.level = None;
            }
        } else if line.contains("Starting integrated minecraft server") {
            self.singleplayer = true;
            self.server = None;
            self.level = None;
        } else if let Some(name) = level_name(line) {
            if self.singleplayer && self.level.is_none() {
                self.level = Some(name);
            }
        } else if let Some(rest) = after(line, "Setting user: ") {
            let name = rest.trim();
            if !name.is_empty() {
                self.player = Some(name.to_string());
            }
        }
        saved_name(line).map(str::to_string)
    }
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.find(marker).map(|i| &line[i + marker.len()..])
}

/// 单人世界的名字：`ServerLevel[名字]`（新版本）、`Saving chunks for level '名字'/…`、
/// `Preparing level "名字"`（老版本）。
fn level_name(line: &str) -> Option<String> {
    if let Some(rest) = after(line, "ServerLevel[") {
        return rest
            .rsplit_once("]'")
            .or_else(|| rest.split_once(']'))
            .map(|(n, _)| n.to_string());
    }
    if let Some(rest) = after(line, "Saving chunks for level '") {
        return rest.split_once("'/").map(|(n, _)| n.to_string());
    }
    if let Some(rest) = after(line, "Preparing level \"") {
        return rest.split_once('"').map(|(n, _)| n.to_string());
    }
    None
}

/// 日志里的世界名对应 `saves/` 下哪个文件夹：同名就是它；否则只有一个以它开头的就是它。
fn folder_for_level(game_dir: &Path, level: &str) -> String {
    let saves = game_dir.join("saves");
    if saves.join(level).join("level.dat").is_file() {
        return level.to_string();
    }
    let candidates: Vec<String> = std::fs::read_dir(&saves)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(level))
        .collect();
    match candidates.as_slice() {
        [only] => only.clone(),
        _ => level.to_string(),
    }
}

/// 现在开着的单人世界：`session.lock` 在世界打开时写入，取这局游戏开始后最新的那个。
fn open_world(game_dir: &Path, since: SystemTime) -> Option<String> {
    std::fs::read_dir(game_dir.join("saves"))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let modified = entry
                .path()
                .join("session.lock")
                .metadata()
                .ok()?
                .modified()
                .ok()?;
            (modified >= since).then(|| (modified, entry.file_name().into_string().ok()))
        })
        .max_by_key(|(modified, _)| *modified)
        .and_then(|(_, name)| name)
}

/// 跟着游戏输出走：看到保存截图的那行，就把时间地点人物记到那张图上。
pub struct Watcher {
    game_dir: PathBuf,
    root: Root,
    tag: Tag,
    started: SystemTime,
    context: Mutex<Context>,
}

impl Watcher {
    pub fn new(root: &Root, game_dir: &Path, tag: Tag) -> Self {
        Self {
            game_dir: game_dir.to_path_buf(),
            root: root.clone(),
            tag,
            // 往前留一点余量：世界可能在启动器记下时间之前一瞬间打开。
            started: SystemTime::now() - std::time::Duration::from_secs(5),
            context: Mutex::new(Context::default()),
        }
    }

    pub fn line(&self, line: &str) {
        let saved = {
            let Ok(mut context) = self.context.lock() else {
                return;
            };
            context.feed(line).map(|name| (name, context.clone()))
        };
        let Some((name, context)) = saved else {
            return;
        };
        let world = if context.singleplayer {
            open_world(&self.game_dir, self.started).or_else(|| {
                context
                    .level
                    .as_deref()
                    .map(|l| folder_for_level(&self.game_dir, l))
            })
        } else {
            None
        };
        let meta = Meta {
            version: Some(self.tag.version.clone()),
            world,
            server: context.server.clone(),
            player: Some(self.tag.player.clone()),
            player_uuid: Some(self.tag.player_uuid.clone()),
            account: Some(self.tag.account.clone()),
            source: Some("live".into()),
        };
        let path = self.game_dir.join("screenshots").join(&name);
        let root = self.root.clone();
        let embed = self.tag.embed;
        // 游戏写完文件才发这条消息，但还是留点余地，也别卡住读日志的线程。
        std::thread::spawn(move || {
            for attempt in 0..10 {
                if embed && write_meta(&path, &meta).is_ok() {
                    return;
                }
                if !embed && path.is_file() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(150 * (attempt + 1)));
            }
            remember(&root, &path, &meta);
        });
    }
}

// ---------------------------------------------------------------- shots.json

/// 记在数据目录里的：从日志推断的、以及玩家关掉「写进图片」时实时记下的。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    shots: BTreeMap<String, Meta>,
    /// 已经读过的日志文件和当时的大小，读过的不再读。
    #[serde(default)]
    logs: BTreeMap<String, u64>,
}

static INDEX_LOCK: Mutex<()> = Mutex::new(());

fn index_path(root: &Root) -> PathBuf {
    root.dir().join("shots.json")
}

fn load_index(root: &Root) -> Index {
    std::fs::read(index_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_index(root: &Root, index: &Index) {
    if let Ok(bytes) = serde_json::to_vec(index) {
        let _ = crate::paths::write_atomic(&index_path(root), &bytes);
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 记到 `shots.json`。已经有实时或手动记下的就不让推断的覆盖。
fn remember(root: &Root, path: &Path, meta: &Meta) {
    let Ok(_guard) = INDEX_LOCK.lock() else {
        return;
    };
    let mut index = load_index(root);
    index.shots.insert(key(path), meta.clone());
    save_index(root, &index);
}

/// 翻一个游戏目录的日志，推断以前那些截图的地点和人物。读过的日志不再读。
fn infer_from_logs(game_dir: &Path, index: &mut Index) -> bool {
    let logs = game_dir.join("logs");
    let Ok(entries) = std::fs::read_dir(&logs) else {
        return false;
    };
    let mut files: Vec<(String, PathBuf, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let wanted = name == "latest.log"
                || (name.ends_with(".log.gz")
                    && name.as_bytes().first().is_some_and(u8::is_ascii_digit));
            let size = entry.metadata().ok()?.len();
            wanted.then(|| (name, entry.path(), size))
        })
        .collect();
    // 日期名按时间排，latest.log 最后。
    files.sort_by(|a, b| {
        (a.0 == "latest.log", log_order(&a.0)).cmp(&(b.0 == "latest.log", log_order(&b.0)))
    });
    let screenshots = game_dir.join("screenshots");
    let mut changed = false;
    for (_, path, size) in files {
        let log_key = key(&path);
        if index.logs.get(&log_key) == Some(&size) {
            continue;
        }
        let Some(text) = read_log(&path) else {
            continue;
        };
        let mut context = Context::default();
        for line in text.lines() {
            let Some(name) = context.feed(line) else {
                continue;
            };
            let shot = key(&screenshots.join(&name));
            if index
                .shots
                .get(&shot)
                .is_some_and(|m| m.source.as_deref() != Some("log"))
            {
                continue;
            }
            let meta = Meta {
                version: None,
                world: if context.singleplayer {
                    context
                        .level
                        .as_deref()
                        .map(|l| folder_for_level(game_dir, l))
                } else {
                    None
                },
                server: context.server.clone(),
                player: context.player.clone(),
                player_uuid: None,
                account: None,
                source: Some("log".into()),
            };
            if meta.world.is_some() || meta.server.is_some() || meta.player.is_some() {
                index.shots.insert(shot, meta);
            }
        }
        index.logs.insert(log_key, size);
        changed = true;
    }
    changed
}

/// `2026-11-04-3.log.gz` → (2026-11-04, 3)，让第 10 个排在第 9 个后面。
fn log_order(name: &str) -> (String, u32) {
    let stem = name.trim_end_matches(".log.gz");
    match stem.rsplit_once('-') {
        Some((date, n)) => (date.to_string(), n.parse().unwrap_or(0)),
        None => (stem.to_string(), 0),
    }
}

fn read_log(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    if path.extension().is_some_and(|e| e == "gz") {
        flate2::read::GzDecoder::new(file)
            .take(256 << 20)
            .read_to_end(&mut bytes)
            .ok()?;
    } else {
        file.take(256 << 20).read_to_end(&mut bytes).ok()?;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------- 列出

/// 一张截图。
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub path: PathBuf,
    /// 它所在的游戏目录（`screenshots/` 的上一级）。
    pub game_dir: PathBuf,
    /// 本地时间：文件名里的，没有就是修改时间。
    pub time: NaiveDateTime,
    pub size: u64,
    pub meta: Meta,
}

/// 列出这些游戏目录里的全部截图，最新的在前。先翻一遍还没读过的日志。
pub fn scan(root: &Root, game_dirs: &[PathBuf]) -> Vec<Shot> {
    let index = {
        let Ok(_guard) = INDEX_LOCK.lock() else {
            return Vec::new();
        };
        let mut index = load_index(root);
        let mut changed = false;
        for dir in game_dirs {
            changed |= infer_from_logs(dir, &mut index);
        }
        if changed {
            save_index(root, &index);
        }
        index
    };
    let mut out = Vec::new();
    for dir in game_dirs {
        let Ok(entries) = std::fs::read_dir(dir.join("screenshots")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".png") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let time = time_from_name(&name)
                .or_else(|| metadata.modified().ok().map(local))
                .unwrap_or_default();
            let mut meta = read_meta(&path).unwrap_or_default();
            if let Some(known) = index.shots.get(&key(&path)) {
                meta.fill_from(known);
            }
            out.push(Shot {
                path,
                game_dir: dir.clone(),
                time,
                size: metadata.len(),
                meta,
            });
        }
    }
    out.sort_by(|a, b| b.time.cmp(&a.time).then_with(|| b.path.cmp(&a.path)));
    out
}

/// 玩家手动指定一张图的地点或人物。能写进图片就写进图片，不能就记在 `shots.json`。
pub fn assign(root: &Root, path: &Path, meta: &Meta, embed: bool) -> Result<()> {
    let mut meta = meta.clone();
    meta.source = Some("manual".into());
    if embed && write_meta(path, &meta).is_ok() {
        return Ok(());
    }
    remember(root, path, &meta);
    Ok(())
}

// ---------------------------------------------------------------- 服务器名

/// `servers.dat` 里的服务器名：地址 → 玩家起的名字（例如 `mc.hypixel.net` → `Hypixel`）。
pub fn server_names(game_dir: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(bytes) = std::fs::read(game_dir.join("servers.dat")) else {
        return out;
    };
    let mut reader = Nbt {
        bytes: &bytes,
        at: 0,
    };
    let mut compounds = Vec::new();
    if reader.u8() == Some(10) && reader.string().is_some() {
        reader.compound(&mut compounds, 0);
    }
    for fields in compounds {
        if let (Some(ip), Some(name)) = (fields.get("ip"), fields.get("name")) {
            let ip = ip.trim().trim_end_matches(":25565").to_string();
            if !ip.is_empty() && !name.trim().is_empty() {
                out.insert(ip, name.trim().to_string());
            }
        }
    }
    out
}

/// 只认字符串的极简 NBT 读法：每个复合标签里的字符串字段收成一张表。
struct Nbt<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Nbt<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn i32(&mut self) -> Option<i32> {
        self.take(4)
            .map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Option<String> {
        let len = self
            .take(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)?;
        self.take(len)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
    fn compound(&mut self, out: &mut Vec<HashMap<String, String>>, depth: u32) -> Option<()> {
        if depth > 16 {
            return None;
        }
        let mut fields = HashMap::new();
        loop {
            let kind = self.u8()?;
            if kind == 0 {
                break;
            }
            let name = self.string()?;
            if kind == 8 {
                fields.insert(name, self.string()?);
            } else {
                self.payload(kind, out, depth)?;
            }
        }
        out.push(fields);
        Some(())
    }
    fn payload(
        &mut self,
        kind: u8,
        out: &mut Vec<HashMap<String, String>>,
        depth: u32,
    ) -> Option<()> {
        match kind {
            1 => self.take(1).map(drop),
            2 => self.take(2).map(drop),
            3 | 5 => self.take(4).map(drop),
            4 | 6 => self.take(8).map(drop),
            7 => {
                let n = usize::try_from(self.i32()?).ok()?;
                self.take(n).map(drop)
            }
            8 => self.string().map(drop),
            9 => {
                let inner = self.u8()?;
                let n = self.i32()?.max(0);
                for _ in 0..n {
                    self.payload(inner, out, depth + 1)?;
                }
                Some(())
            }
            10 => self.compound(out, depth + 1),
            11 => {
                let n = usize::try_from(self.i32()?).ok()?;
                self.take(n.checked_mul(4)?).map(drop)
            }
            12 => {
                let n = usize::try_from(self.i32()?).ok()?;
                self.take(n.checked_mul(8)?).map(drop)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1×1 的灰色 PNG。
    fn tiny_png() -> Vec<u8> {
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            let mut crc = crc32fast::Hasher::new();
            crc.update(kind);
            crc.update(data);
            out.extend_from_slice(&crc.finalize().to_be_bytes());
        }
        let mut out = SIGNATURE.to_vec();
        chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &[0, 128]).unwrap();
        chunk(&mut out, b"IDAT", &encoder.finish().unwrap());
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cairn-shots-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn meta_round_trips_through_the_png() {
        let dir = temp_dir("png");
        let path = dir.join("2026-11-07_21.34.12.png");
        let original = tiny_png();
        std::fs::write(&path, &original).unwrap();
        let old_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
        assert_eq!(read_meta(&path), None);
        let meta = Meta {
            world: Some("生存一号".into()),
            player: Some("Steve".into()),
            source: Some("live".into()),
            ..Default::default()
        };
        write_meta(&path, &meta).unwrap();
        assert_eq!(read_meta(&path), Some(meta.clone()));
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            old_time,
            "保留修改时间"
        );
        // 再写一次是替换，不是叠加；其余块原样保留。
        let second = Meta {
            world: Some("空岛".into()),
            ..meta
        };
        write_meta(&path, &second).unwrap();
        assert_eq!(read_meta(&path), Some(second));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.windows(4).filter(|w| w == b"iTXt").count(), 1);
        assert!(
            bytes.ends_with(&original[original.len() - 12..]),
            "IEND 还在最后"
        );
        let idat = |b: &[u8]| {
            b.windows(4)
                .position(|w| w == b"IDAT")
                .map(|i| b[i..].to_vec())
        };
        assert_eq!(idat(&bytes), idat(&original), "像素数据一个字节都没动");
        assert!(write_meta(&dir.join("missing.png"), &Meta::default()).is_err());
        std::fs::write(dir.join("broken.png"), &original[..20]).unwrap();
        assert!(write_meta(&dir.join("broken.png"), &Meta::default()).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_time_from_the_file_name() {
        let t = time_from_name("2026-11-07_21.34.12.png").unwrap();
        assert_eq!(t.to_string(), "2026-11-07 21:34:12");
        assert!(time_from_name("2026-11-07_21.34.12_2.png").is_some());
        assert!(time_from_name("2026-11-07_21.34.12_x.png").is_none());
        assert!(time_from_name("panorama_0.png").is_none());
        assert!(time_from_name("2026-13-07_21.34.12.png").is_none());
    }

    #[test]
    fn finds_saved_screenshots_in_any_language() {
        let en = "[21:34:12] [Render thread/INFO]: [System] [CHAT] Saved screenshot as 2026-11-07_21.34.12.png";
        let zh = "[21:34:12] [Render thread/INFO]: [System] [CHAT] 已将截图保存为2026-11-07_21.34.12_2.png";
        let old =
            "[12:00:00] [Client thread/INFO]: [CHAT] Saved screenshot as 2017-06-01_12.00.00.png";
        assert_eq!(saved_name(en), Some("2026-11-07_21.34.12.png"));
        assert_eq!(saved_name(zh), Some("2026-11-07_21.34.12_2.png"));
        assert_eq!(saved_name(old), Some("2017-06-01_12.00.00.png"));
        assert_eq!(
            saved_name("[main/INFO]: loaded 2026-11-07_21.34.12.png"),
            None,
            "不是聊天行"
        );
        assert_eq!(saved_name("[CHAT] see icon.png"), None);
    }

    #[test]
    fn follows_worlds_and_servers_through_a_log() {
        let log = "\
[main/INFO]: Setting user: Steve
[Server thread/INFO]: Starting integrated minecraft server version 26.3
[Server thread/INFO]: Saving chunks for level 'ServerLevel[生存一号]'/minecraft:overworld
[Render thread/INFO]: [System] [CHAT] Saved screenshot as 2026-11-07_21.34.12.png
[Render thread/INFO]: Connecting to mc.hypixel.net, 25565
[Render thread/INFO]: [System] [CHAT] Saved screenshot as 2026-11-07_21.50.00.png
[Render thread/INFO]: Connecting to 127.0.0.1, 25570
[Render thread/INFO]: [System] [CHAT] Saved screenshot as 2026-11-07_22.00.00.png
[Server thread/INFO]: Preparing level \"Old World\"
[Server thread/INFO]: Starting integrated minecraft server version 1.12.2
[Server thread/INFO]: Preparing level \"Old World\"
[Client thread/INFO]: [CHAT] Saved screenshot as 2026-11-07_23.00.00.png
";
        let game_dir = temp_dir("log");
        std::fs::create_dir_all(game_dir.join("logs")).unwrap();
        std::fs::create_dir_all(game_dir.join("saves/生存一号")).unwrap();
        std::fs::write(game_dir.join("saves/生存一号/level.dat"), b"x").unwrap();
        std::fs::write(game_dir.join("logs/latest.log"), log).unwrap();
        let mut index = Index::default();
        assert!(infer_from_logs(&game_dir, &mut index));
        let get = |name: &str| {
            index
                .shots
                .get(&key(&game_dir.join("screenshots").join(name)))
                .cloned()
                .unwrap()
        };
        let first = get("2026-11-07_21.34.12.png");
        assert_eq!(first.world.as_deref(), Some("生存一号"));
        assert_eq!(first.player.as_deref(), Some("Steve"));
        assert_eq!(first.source.as_deref(), Some("log"));
        assert_eq!(
            get("2026-11-07_21.50.00.png").server.as_deref(),
            Some("mc.hypixel.net")
        );
        assert_eq!(
            get("2026-11-07_22.00.00.png").server.as_deref(),
            Some("127.0.0.1:25570")
        );
        let old = get("2026-11-07_23.00.00.png");
        assert_eq!(old.world.as_deref(), Some("Old World"));
        assert_eq!(old.server, None);
        assert!(!infer_from_logs(&game_dir, &mut index), "读过的日志不再读");
        let _ = std::fs::remove_dir_all(game_dir);
    }

    #[test]
    fn the_watcher_tags_screenshots_as_they_are_taken() {
        let root_dir = temp_dir("watch");
        let root = Root::new(&root_dir);
        let game_dir = root_dir.join("versions/a");
        std::fs::create_dir_all(game_dir.join("screenshots")).unwrap();
        std::fs::create_dir_all(game_dir.join("saves/老世界")).unwrap();
        std::fs::write(game_dir.join("saves/老世界/session.lock"), "x").unwrap();
        // 老世界的 session.lock 早于这局游戏，不算；这局打开的是新世界。
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(game_dir.join("saves/老世界/session.lock"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let tag = Tag {
            version: "Fabric 1.21.1".into(),
            player: "Steve".into(),
            player_uuid: "abc".into(),
            account: "正版".into(),
            embed: true,
        };
        let watcher = Watcher::new(&root, &game_dir, tag.clone());
        std::fs::create_dir_all(game_dir.join("saves/新世界")).unwrap();
        std::fs::write(game_dir.join("saves/新世界/session.lock"), "x").unwrap();
        let first = game_dir.join("screenshots/2026-11-07_21.34.12.png");
        let second = game_dir.join("screenshots/2026-11-07_21.40.00.png");
        std::fs::write(&first, tiny_png()).unwrap();
        std::fs::write(&second, tiny_png()).unwrap();
        watcher.line("[Server thread/INFO]: Starting integrated minecraft server version 1.21.1");
        watcher.line(
            "[Render thread/INFO]: [System] [CHAT] Saved screenshot as 2026-11-07_21.34.12.png",
        );
        watcher.line("[Render thread/INFO]: Connecting to mc.hypixel.net, 25565");
        watcher.line("[Render thread/INFO]: [System] [CHAT] 已将截图保存为2026-11-07_21.40.00.png");
        let wait = |path: &Path| {
            for _ in 0..100 {
                if let Some(meta) = read_meta(path) {
                    return meta;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("没写进去：{}", path.display());
        };
        let a = wait(&first);
        assert_eq!(a.world.as_deref(), Some("新世界"));
        assert_eq!(a.player.as_deref(), Some("Steve"));
        assert_eq!(a.version.as_deref(), Some("Fabric 1.21.1"));
        assert_eq!(a.source.as_deref(), Some("live"));
        let b = wait(&second);
        assert_eq!(b.world, None);
        assert_eq!(b.server.as_deref(), Some("mc.hypixel.net"));
        // 关掉「写进图片」时记在 shots.json，图片不动。
        let third = game_dir.join("screenshots/2026-11-07_22.00.00.png");
        std::fs::write(&third, tiny_png()).unwrap();
        let quiet = Watcher::new(
            &root,
            &game_dir,
            Tag {
                embed: false,
                ..tag
            },
        );
        quiet.line(
            "[Render thread/INFO]: [System] [CHAT] Saved screenshot as 2026-11-07_22.00.00.png",
        );
        for _ in 0..100 {
            if load_index(&root).shots.contains_key(&key(&third)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(read_meta(&third), None);
        assert_eq!(
            load_index(&root).shots[&key(&third)].player.as_deref(),
            Some("Steve")
        );
        let _ = std::fs::remove_dir_all(root_dir);
    }

    #[test]
    fn scan_prefers_what_the_image_says() {
        let root_dir = temp_dir("scan");
        let root = Root::new(&root_dir);
        let game_dir = root_dir.join("versions/a");
        let shots = game_dir.join("screenshots");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::write(shots.join("2026-11-07_21.34.12.png"), tiny_png()).unwrap();
        std::fs::write(shots.join("2026-11-06_08.00.00.png"), tiny_png()).unwrap();
        std::fs::write(shots.join("notes.txt"), "x").unwrap();
        write_meta(
            &shots.join("2026-11-07_21.34.12.png"),
            &Meta {
                world: Some("生存一号".into()),
                source: Some("live".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assign(
            &root,
            &shots.join("2026-11-06_08.00.00.png"),
            &Meta {
                player: Some("Alex".into()),
                ..Default::default()
            },
            false,
        )
        .unwrap();
        let list = scan(&root, std::slice::from_ref(&game_dir));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].meta.world.as_deref(), Some("生存一号"));
        assert_eq!(list[0].time.to_string(), "2026-11-07 21:34:12");
        assert_eq!(list[1].meta.player.as_deref(), Some("Alex"));
        assert_eq!(list[1].meta.source.as_deref(), Some("manual"));
        let _ = std::fs::remove_dir_all(root_dir);
    }

    #[test]
    fn reads_server_names() {
        // { "": { servers: [ { ip: "mc.hypixel.net", name: "Hypixel", icon: "..." } ] } }
        let mut b = vec![10, 0, 0];
        b.extend_from_slice(&[9, 0, 7]);
        b.extend_from_slice(b"servers");
        b.push(10);
        b.extend_from_slice(&1i32.to_be_bytes());
        let string = |b: &mut Vec<u8>, name: &str, value: &str| {
            b.push(8);
            b.extend_from_slice(&(name.len() as u16).to_be_bytes());
            b.extend_from_slice(name.as_bytes());
            b.extend_from_slice(&(value.len() as u16).to_be_bytes());
            b.extend_from_slice(value.as_bytes());
        };
        string(&mut b, "ip", "mc.hypixel.net:25565");
        b.push(1); // 字节标签：acceptTextures = 1
        b.extend_from_slice(&14u16.to_be_bytes());
        b.extend_from_slice(b"acceptTextures");
        b.push(1);
        string(&mut b, "name", "Hypixel");
        b.push(0);
        b.push(0);
        let dir = temp_dir("servers");
        std::fs::write(dir.join("servers.dat"), &b).unwrap();
        let names = server_names(&dir);
        assert_eq!(
            names.get("mc.hypixel.net").map(String::as_str),
            Some("Hypixel")
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
