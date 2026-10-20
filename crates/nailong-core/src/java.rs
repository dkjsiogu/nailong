use crate::error::{Error, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct JavaRuntime {
    pub path: PathBuf,
    pub major: u32,
    pub version: String,
    pub vendor: Option<String>,
    pub arch: Option<String>,
}

impl JavaRuntime {
    pub fn label(&self) -> String {
        match &self.vendor {
            Some(v) => format!("Java {} ({v})", self.version),
            None => format!("Java {}", self.version),
        }
    }
}

pub fn find_all() -> Vec<JavaRuntime> {
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(java_exe()));
    }
    if let Some(path) = which_java() {
        candidates.push(path);
    }
    #[cfg(unix)]
    {
        for base in ["/usr/lib/jvm", "/usr/java", "/opt/java"] {
            if let Ok(entries) = std::fs::read_dir(base) {
                for e in entries.flatten() {
                    let p = e.path().join("bin").join("java");
                    if p.exists() {
                        candidates.push(p);
                    }
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let base = PathBuf::from(home).join(".jdks");
            if let Ok(entries) = std::fs::read_dir(base) {
                for e in entries.flatten() {
                    let p = e.path().join("bin").join("java");
                    if p.exists() {
                        candidates.push(p);
                    }
                }
            }
        }
    }
    #[cfg(windows)]
    {
        for env in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(env) {
                let base = PathBuf::from(base);
                for sub in [
                    "Java",
                    "Eclipse Adoptium",
                    "Microsoft",
                    "Zulu",
                    "Amazon Corretto",
                    "BellSoft",
                ] {
                    let dir = base.join(sub);
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for e in entries.flatten() {
                            let p = e.path().join("bin").join("java.exe");
                            if p.exists() {
                                candidates.push(p);
                            }
                        }
                    }
                }
                let dir = base.join("Programs").join("Eclipse Adoptium");
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for e in entries.flatten() {
                        let p = e.path().join("bin").join("java.exe");
                        if p.exists() {
                            candidates.push(p);
                        }
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in candidates {
        let key = std::fs::canonicalize(&c).unwrap_or_else(|_| c.clone());
        if !seen.insert(key) {
            continue;
        }
        if let Ok(rt) = probe(&c) {
            out.push(rt);
        }
    }
    out.sort_by(|a, b| b.major.cmp(&a.major).then(a.path.cmp(&b.path)));
    out
}

pub fn probe(path: &Path) -> Result<JavaRuntime> {
    if !path.exists() {
        return Err(Error::JavaNotFound(0));
    }
    let home = path.parent().and_then(|p| p.parent()).unwrap_or(path);
    if let Some(rt) = probe_release(home, path) {
        return Ok(rt);
    }
    let output = Command::new(path)
        .args(["-XshowSettings:properties", "-version"])
        .output()
        .map_err(|_| Error::JavaNotFound(0))?;
    let text = String::from_utf8_lossy(&output.stderr);
    let mut version = None;
    let mut vendor = None;
    let mut arch = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("java.version =") {
            version = Some(v.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("java.vendor =") {
            vendor = Some(v.trim().to_string());
        }
        if let Some(v) = line.strip_prefix("os.arch =") {
            arch = Some(v.trim().to_string());
        }
    }
    let version = version.ok_or(Error::JavaNotFound(0))?;
    let major = major_from_version(&version);
    Ok(JavaRuntime {
        path: path.to_path_buf(),
        major,
        version,
        vendor,
        arch,
    })
}

fn probe_release(home: &Path, path: &Path) -> Option<JavaRuntime> {
    let release = std::fs::read_to_string(home.join("release")).ok()?;
    let mut version = None;
    let mut arch = None;
    let mut vendor = None;
    for line in release.lines() {
        let mut parts = line.splitn(2, '=');
        let k = parts.next()?.trim();
        let v = parts.next().unwrap_or("").trim().trim_matches('"');
        match k {
            "JAVA_VERSION" => version = Some(v.to_string()),
            "OS_ARCH" => arch = Some(v.to_string()),
            "IMPLEMENTOR" => vendor = Some(v.to_string()),
            _ => {}
        }
    }
    let version = version?;
    Some(JavaRuntime {
        path: path.to_path_buf(),
        major: major_from_version(&version),
        version,
        vendor,
        arch,
    })
}

pub fn major_from_version(v: &str) -> u32 {
    if let Some(rest) = v.strip_prefix("1.") {
        rest.split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8)
    } else {
        v.split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

pub fn select(required: u32, candidates: &[JavaRuntime]) -> Result<JavaRuntime> {
    if let Some(j) = candidates.iter().find(|j| j.major == required) {
        return Ok(j.clone());
    }
    if let Some(j) = candidates.iter().find(|j| j.major > required) {
        return Ok(j.clone());
    }
    Err(Error::JavaNotFound(required))
}

fn which_java() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(java_exe());
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn java_exe() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}
