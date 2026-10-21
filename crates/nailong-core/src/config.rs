use crate::error::{Result, io};
use crate::paths::Root;
#[cfg(not(unix))]
use crate::paths::write_atomic;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Small launcher-level settings file, stored next to the game data.
/// It deliberately contains no tokens beyond the optional CurseForge key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub curseforge_api_key: Option<String>,
}

pub fn path(root: &Root) -> PathBuf {
    root.dir().join("nailong.json")
}

pub fn load(root: &Root) -> Result<Config> {
    let path = path(root);
    if !path.is_file() {
        return Ok(Config::default());
    }
    let bytes = std::fs::read(&path).map_err(|e| io(&path, e))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| crate::error::Error::Install(format!("设置文件解析失败: {e}")))
}

pub fn save(root: &Root, config: &Config) -> Result<()> {
    let path = path(root);
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| crate::error::Error::Install(format!("设置序列化失败: {e}")))?;
    write_private(&path, &bytes)
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| io(&tmp, e))?;
    file.write_all(bytes).map_err(|e| io(&tmp, e))?;
    file.sync_all().map_err(|e| io(&tmp, e))?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(path, e)
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    write_atomic(path, bytes)
}

/// Build-time fallback for release builds. Set `NAILONG_CURSEFORGE_API_KEY`
/// in CI to bake in a project key; an empty variable is ignored. The key is
/// still extractable from a desktop binary, so runtime/user values win.
const BUILTIN_CURSEFORGE_API_KEY: Option<&str> = option_env!("NAILONG_CURSEFORGE_API_KEY");

pub fn builtin_curseforge_key() -> Option<String> {
    BUILTIN_CURSEFORGE_API_KEY
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Priority: runtime environment, saved setting, then the build-time default.
pub fn curseforge_api_key(root: &Root) -> Option<String> {
    std::env::var("CURSEFORGE_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            load(root)
                .ok()
                .and_then(|config| config.curseforge_api_key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(builtin_curseforge_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> Root {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Root::new(std::env::temp_dir().join(format!("nailong-config-{}-{n}", std::process::id())))
    }

    #[test]
    fn saves_and_loads_key() {
        let root = temp_root();
        let config = Config {
            curseforge_api_key: Some("test-key".into()),
        };
        save(&root, &config).unwrap();
        let loaded = load(&root).unwrap();
        assert_eq!(loaded.curseforge_api_key.as_deref(), Some("test-key"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&root)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(root.dir());
    }
}
