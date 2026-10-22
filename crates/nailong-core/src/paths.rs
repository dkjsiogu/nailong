use crate::error::{Result, io};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Root {
    dir: PathBuf,
}

impl Root {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_root() -> PathBuf {
        // Windows: portable by default — game data lives next to nailong.exe.
        #[cfg(windows)]
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            return dir.join(".minecraft");
        }
        directories::BaseDirs::new()
            .map(|d| d.data_dir().join("nailong").join(".minecraft"))
            .unwrap_or_else(|| PathBuf::from(".minecraft"))
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
        self.dir.join("cache").join("nailong")
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
