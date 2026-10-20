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

    pub fn ensure(&self) -> Result<()> {
        for p in [
            self.dir.clone(),
            self.versions(),
            self.libraries(),
            self.assets(),
            self.indexes(),
            self.objects(),
            self.cache(),
        ] {
            std::fs::create_dir_all(&p).map_err(|e| io(&p, e))?;
        }
        Ok(())
    }
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
