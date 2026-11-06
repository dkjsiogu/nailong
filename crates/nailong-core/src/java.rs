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

    /// 读不到架构时按 64 位处理，现在的发行版基本都是。
    pub fn is_64bit(&self) -> bool {
        let Some(arch) = &self.arch else {
            return true;
        };
        !matches!(
            arch.to_ascii_lowercase().as_str(),
            "x86" | "i386" | "i486" | "i586" | "i686" | "arm" | "aarch32" | "ppc" | "sparc"
        )
    }

    /// Oracle GraalVM 的 vendor 也写 Oracle，所以还要看 `release` 文件。
    pub fn is_graal(&self) -> bool {
        let lower = |s: &str| s.to_ascii_lowercase().contains("graal");
        if lower(&self.path.to_string_lossy()) || self.vendor.as_deref().is_some_and(lower) {
            return true;
        }
        let home = self.path.parent().and_then(|p| p.parent());
        home.and_then(|home| std::fs::read_to_string(home.join("release")).ok())
            .is_some_and(|release| release.contains("GRAALVM_VERSION"))
    }
}

/// 本机能找到的所有 Java：系统里装的，加上启动器自己下载到数据目录的。
pub fn find_all(root: &crate::paths::Root) -> Vec<JavaRuntime> {
    let mut candidates = crate::runtime::installed(root);
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
        // 只取开头的数字：`21.0.7`、`25-ea`，以及 Mojang 运行时清单里的 `8u51`。
        let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().unwrap_or(0)
    }
}

/// 选最接近要求的 Java：版本不够的不选，够的里面选最低的那个大版本。
/// 老版本（尤其是 Forge 1.12 的 LaunchWrapper）在太新的 Java 上起不来，所以不能直接拿最新的。
/// 同一个大版本里优先普通的 64 位 OpenJDK：GraalVM 换 ZGC 时会关掉自己的编译器。
pub fn select(required: u32, candidates: &[JavaRuntime]) -> Result<JavaRuntime> {
    candidates
        .iter()
        .filter(|j| j.major >= required)
        .min_by_key(|j| (j.major, !j.is_64bit(), j.is_graal()))
        .cloned()
        .ok_or(Error::JavaNotFound(required))
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

#[cfg(test)]
mod select_tests {
    use super::*;

    fn rt(major: u32, path: &str, arch: &str) -> JavaRuntime {
        JavaRuntime {
            path: PathBuf::from(path),
            major,
            version: major.to_string(),
            vendor: None,
            arch: Some(arch.into()),
        }
    }

    #[test]
    fn picks_the_closest_newer_java_and_skips_graal() {
        let all = [
            rt(25, "/usr/lib/jvm/java-25/bin/java", "amd64"),
            rt(21, "/usr/lib/jvm/graalvm-21/bin/java", "amd64"),
            rt(21, "/usr/lib/jvm/java-21/bin/java", "amd64"),
            rt(17, "/usr/lib/jvm/java-17/bin/java", "x86"),
            rt(17, "/usr/lib/jvm/java-17-64/bin/java", "amd64"),
        ];
        assert_eq!(
            select(21, &all).unwrap().path,
            PathBuf::from("/usr/lib/jvm/java-21/bin/java")
        );
        // 要 Java 8 而本机没有时，用最接近的 17，而不是最新的 25。
        assert_eq!(
            select(8, &all).unwrap().path,
            PathBuf::from("/usr/lib/jvm/java-17-64/bin/java")
        );
        assert_eq!(select(22, &all).unwrap().major, 25);
        assert!(select(26, &all).is_err());
    }
}
