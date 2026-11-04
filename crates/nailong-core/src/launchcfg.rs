//! 每个版本自己的启动参数：Java、内存、回收器、额外 JVM 参数、窗口大小。
//! 没改的项跟随全局设置；全部跟随时不占文件里的位置。存在 `<数据目录>/launch-settings.json`。

use crate::error::{Error, Result, io};
use crate::paths::Root;
use crate::tuning::GcChoice;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

const STORE: &str = "launch-settings.json";
/// 不单独设置时的游戏窗口大小。
pub const DEFAULT_WINDOW: [u32; 2] = [1280, 720];

/// 这个版本单独的内存设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryPick {
    /// 按电脑和实测自动算。
    Auto,
    /// 固定的最大堆，MB。
    Mb(u32),
}

impl MemoryPick {
    /// 换成启动选项里的写法：None 表示自动。
    pub fn memory_mb(self) -> Option<u32> {
        match self {
            Self::Auto => None,
            Self::Mb(mb) => Some(mb),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionLaunch {
    /// 指定的 Java 程序；None 时自动选。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryPick>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc: Option<GcChoice>,
    /// 额外 JVM 参数；None 时跟随全局。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jvm_args: Option<String>,
    /// 游戏窗口宽高；None 时用默认的 1280×720。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<[u32; 2]>,
}

impl VersionLaunch {
    /// 全部跟随全局。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn window(&self) -> [u32; 2] {
        self.window.unwrap_or(DEFAULT_WINDOW)
    }
}

fn store_path(root: &Root) -> PathBuf {
    root.dir().join(STORE)
}

pub fn load_all(root: &Root) -> BTreeMap<String, VersionLaunch> {
    std::fs::read(store_path(root))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 保存一个版本的设置；全部跟随全局时从文件里删掉这一项。
pub fn save(root: &Root, id: &str, settings: &VersionLaunch) -> Result<()> {
    let mut all = load_all(root);
    if settings.is_default() {
        all.remove(id);
    } else {
        all.insert(id.to_string(), settings.clone());
    }
    let path = store_path(root);
    let bytes = serde_json::to_vec_pretty(&all).map_err(|e| Error::Launch(e.to_string()))?;
    // 先写临时文件再改名：写到一半断电也不会留下半个 JSON。
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| io(&path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_only_what_differs_from_global() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cairn-launchcfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = Root::new(&dir);
        let pack = VersionLaunch {
            java: Some(PathBuf::from("/opt/java-21/bin/java")),
            memory: Some(MemoryPick::Mb(6144)),
            gc: Some(GcChoice::Zgc),
            jvm_args: None,
            window: Some([1920, 1080]),
        };
        save(&root, "pack", &pack).unwrap();
        save(&root, "vanilla", &VersionLaunch::default()).unwrap();
        let all = load_all(&root);
        assert_eq!(all.get("pack"), Some(&pack));
        assert!(!all.contains_key("vanilla"));
        let text = std::fs::read_to_string(dir.join(STORE)).unwrap();
        assert!(text.contains("\"zgc\""));
        assert!(!text.contains("jvm_args"));
        save(&root, "pack", &VersionLaunch::default()).unwrap();
        assert!(load_all(&root).is_empty());
        assert_eq!(MemoryPick::Auto.memory_mb(), None);
        assert_eq!(VersionLaunch::default().window(), DEFAULT_WINDOW);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
