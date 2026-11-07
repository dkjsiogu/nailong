use crate::error::{Error, Result, io};
use std::path::{Path, PathBuf};

/// 数据目录里的程序名。0.37 起叫 Cairn，以前叫 nailong。
const APP_DIR: &str = "cairn";
const LEGACY_APP_DIR: &str = "nailong";

/// 旧版的数据在 `<系统数据目录>/nailong`：新目录还不存在时整个搬过来（同一块盘上只是改名）；
/// 搬不动（例如被占用或跨磁盘）就继续用旧目录，什么都不丢。
fn data_home(base: &Path) -> PathBuf {
    let new = base.join(APP_DIR);
    let old = base.join(LEGACY_APP_DIR);
    if !new.exists() && old.is_dir() {
        return if std::fs::rename(&old, &new).is_ok() {
            new
        } else {
            old
        };
    }
    new
}

#[derive(Debug, Clone)]
pub struct Root {
    dir: PathBuf,
}

impl Root {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        // 游戏进程的工作目录是版本目录，相对路径会让类路径全部失效。
        let dir = std::path::absolute(&dir).unwrap_or(dir);
        Self { dir }
    }

    pub fn default_root() -> PathBuf {
        // Windows：默认便携，游戏数据放在程序旁边的 .minecraft，改名不影响。
        #[cfg(windows)]
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            return dir.join(".minecraft");
        }
        directories::BaseDirs::new()
            .map(|d| data_home(d.data_dir()).join(".minecraft"))
            .unwrap_or_else(|| PathBuf::from(".minecraft"))
    }

    /// 0.37 以前缓存在 `cache/nailong`：新目录还没有时直接改名过去。
    pub fn migrate_legacy_cache(&self) {
        let old = self.dir.join("cache").join(LEGACY_APP_DIR);
        let new = self.cache();
        if !new.exists() && old.is_dir() {
            let _ = std::fs::rename(old, new);
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn versions(&self) -> PathBuf {
        self.dir.join("versions")
    }

    pub fn libraries(&self) -> PathBuf {
        self.dir.join("libraries")
    }

    pub fn assets(&self) -> PathBuf {
        self.dir.join("assets")
    }

    pub fn indexes(&self) -> PathBuf {
        self.assets().join("indexes")
    }

    pub fn objects(&self) -> PathBuf {
        self.assets().join("objects")
    }

    pub fn cache(&self) -> PathBuf {
        self.dir.join("cache").join(APP_DIR)
    }

    pub fn instances(&self) -> PathBuf {
        self.dir.join("instances")
    }

    pub fn instance_dir(&self, id: &str) -> PathBuf {
        self.instances().join(sanitize(id))
    }

    pub fn natives(&self, profile: &str) -> PathBuf {
        self.dir.join("natives").join(sanitize(profile))
    }

    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }

    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }

    pub fn client_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }

    /// Present while a Forge/NeoForge processor chain has not completed.
    /// A partial patched client must never be treated as a healthy install.
    pub fn incomplete_marker(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(".nailong-incomplete")
    }

    /// Present when a version has opted into its own game directory (mods,
    /// saves and options no longer live in the shared root).
    pub fn isolated_marker(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(".nailong-isolated")
    }

    /// Remove a version directory by its exact on-disk name.  Only direct
    /// children are accepted, so a profile id can never escape `versions/`.
    pub fn remove_version(&self, id: &str) -> Result<()> {
        remove_child(&self.versions(), id)
    }

    /// Remove an instance directory by its exact on-disk name.
    pub fn remove_instance(&self, id: &str) -> Result<()> {
        remove_child(&self.instances(), id)
    }

    pub fn ensure(&self) -> Result<()> {
        for p in [
            self.dir.clone(),
            self.versions(),
            self.libraries(),
            self.assets(),
            self.indexes(),
            self.objects(),
            self.cache(),
            self.instances(),
        ] {
            std::fs::create_dir_all(&p).map_err(|e| io(&p, e))?;
        }
        Ok(())
    }
}

fn remove_child(parent: &Path, id: &str) -> Result<()> {
    if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
        return Err(crate::error::Error::Install(format!(
            "拒绝删除非法路径: {id}"
        )));
    }
    let path = parent.join(id);
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(crate::error::Error::Install(format!(
            "不是目录，拒绝删除: {}",
            path.display()
        )));
    }
    std::fs::remove_dir_all(&path).map_err(|e| io(&path, e))
}

pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(path, e)
    })?;
    Ok(())
}

/// 移到回收站（不是直接删掉）。Linux 用 `gio trash`，Windows 用系统的回收站。
pub fn to_trash(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            SHFILEOPSTRUCTW, SHFileOperationW,
        };
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        // 路径要以两个 \0 结尾。
        let mut from: Vec<u16> = absolute.as_os_str().encode_wide().collect();
        from.extend_from_slice(&[0, 0]);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as _,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: 结构体里的指针都指向这里活着的缓冲区，调用是同步的。
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 || op.fAnyOperationsAborted != 0 {
            return Err(Error::Install(format!(
                "没能移到回收站（{code}）: {}",
                path.display()
            )));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("gio")
            .arg("trash")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => Ok(()),
            _ => Err(Error::Install(format!(
                "没能移到回收站（需要 gio）: {}",
                path.display()
            ))),
        }
    }
}

#[cfg(test)]
mod root_tests {
    use super::*;

    #[test]
    fn moves_the_old_data_dir_once() {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("cairn-rename-{}-{n}", std::process::id()));
        std::fs::create_dir_all(base.join("nailong/.minecraft/saves/World")).unwrap();
        std::fs::write(base.join("nailong/.minecraft/ui.json"), b"{}").unwrap();
        assert_eq!(data_home(&base), base.join("cairn"));
        assert!(base.join("cairn/.minecraft/saves/World").is_dir());
        assert!(base.join("cairn/.minecraft/ui.json").is_file());
        assert!(!base.join("nailong").exists());
        // 新目录已经在了：不再动旧目录。
        std::fs::create_dir_all(base.join("nailong/.minecraft")).unwrap();
        assert_eq!(data_home(&base), base.join("cairn"));
        assert!(base.join("nailong").exists());
        let root = Root::new(base.join("cairn/.minecraft"));
        std::fs::create_dir_all(root.dir().join("cache/nailong/logs")).unwrap();
        root.migrate_legacy_cache();
        assert!(root.cache().join("logs").is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn relative_roots_become_absolute() {
        let root = Root::new("relative/.minecraft");
        assert!(root.dir().is_absolute());
        assert!(root.dir().ends_with("relative/.minecraft"));
    }
}
