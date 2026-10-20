use crate::error::{Error, Result, json};
use crate::paths::Root;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::MacOs => "osx",
            Os::Linux => "linux",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86,
    X86_64,
    Arm64,
}

impl Arch {
    pub fn current() -> Self {
        if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X86
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Arch::X86 => "x86",
            Arch::X86_64 => "x86_64",
            Arch::Arm64 => "arm64",
        }
    }
    pub fn bits(self) -> &'static str {
        match self {
            Arch::X86 => "32",
            _ => "64",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Features {
    pub demo: bool,
    pub custom_resolution: bool,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub os: Os,
    pub arch: Arch,
    pub os_version: Option<String>,
    pub features: Features,
}

impl Context {
    pub fn current() -> Self {
        Self {
            os: Os::current(),
            arch: Arch::current(),
            os_version: None,
            features: Features::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(rename = "mainClass", default)]
    pub main_class: Option<String>,
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub jar: Option<String>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndex>,
    #[serde(rename = "minecraftArguments", default)]
    pub legacy_arguments: Option<String>,
    #[serde(default)]
    pub arguments: Option<Arguments>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub downloads: Downloads,
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub logging: Option<Logging>,
    #[serde(rename = "minimumLauncherVersion", default)]
    pub minimum_launcher_version: Option<u32>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
    #[serde(rename = "default-user-jvm", default)]
    pub default_user_jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Ruled {
        rules: Vec<Rule>,
        value: StringOrList,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrList {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: Action,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(rename = "versionRange", default)]
    pub version_range: Option<VersionRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionRange {
    #[serde(default)]
    pub min: Option<String>,
    #[serde(default)]
    pub max: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibDownloads>,
    #[serde(default)]
    pub natives: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub extract: Option<Extract>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<BTreeMap<String, Artifact>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Downloads {
    #[serde(default)]
    pub client: Option<DownloadInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadInfo {
    #[serde(default)]
    pub path: Option<String>,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIndex {
    pub id: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(rename = "totalSize", default)]
    pub total_size: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaVersion {
    pub component: String,
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Logging {
    #[serde(default)]
    pub client: Option<LoggingClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LogFile,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogFile {
    pub id: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub id: String,
    pub main_class: String,
    pub client_jar: PathBuf,
    pub client_jar_id: String,
    pub asset_index: Option<AssetIndex>,
    pub libraries: Vec<Library>,
    pub arguments: Arguments,
    pub legacy_arguments: Option<String>,
    pub java_major: Option<u32>,
    pub logging: Option<LoggingClient>,
    pub chain: Vec<String>,
}

pub fn load(root: &Root, id: &str) -> Result<VersionJson> {
    let path = root.version_json(id);
    let data = std::fs::read(&path).map_err(|e| crate::error::io(&path, e))?;
    serde_json::from_slice(&data).map_err(|e| json(&path, e))
}

/// Merge the inheritance chain without requiring the client jar to exist.
/// Used by the health check to report a missing client instead of failing.
pub fn merged_chain(root: &Root, id: &str) -> Result<(VersionJson, Vec<String>)> {
    resolve_inner(root, id, &mut Vec::new())
}

pub fn resolve(root: &Root, id: &str, _ctx: &Context) -> Result<Resolved> {
    let mut stack = Vec::new();
    let (merged, chain) = resolve_inner(root, id, &mut stack)?;
    let main_class = merged
        .main_class
        .clone()
        .ok_or_else(|| Error::NoMainClass(id.to_string()))?;
    let client_jar_id = merged
        .jar
        .clone()
        .or_else(|| {
            chain
                .iter()
                .rev()
                .find(|v| {
                    load(root, v)
                        .ok()
                        .and_then(|m| m.downloads.client)
                        .is_some()
                })
                .cloned()
        })
        .unwrap_or_else(|| chain.first().cloned().unwrap_or_default());
    let client_jar = root.client_jar(&client_jar_id);
    if !client_jar.exists() {
        return Err(Error::MissingFile(client_jar));
    }
    let java_major = merged.java_version.as_ref().map(|j| j.major_version);
    let logging = merged.logging.and_then(|l| l.client);
    Ok(Resolved {
        id: merged.id,
        main_class,
        client_jar,
        client_jar_id,
        asset_index: merged.asset_index,
        libraries: merged.libraries,
        arguments: merged.arguments.unwrap_or_default(),
        legacy_arguments: merged.legacy_arguments,
        java_major,
        logging,
        chain,
    })
}

fn resolve_inner(
    root: &Root,
    id: &str,
    stack: &mut Vec<String>,
) -> Result<(VersionJson, Vec<String>)> {
    if stack.iter().any(|s| s == id) {
        return Err(Error::CircularInheritance(format!(
            "{} -> {id}",
            stack.join(" -> ")
        )));
    }
    if stack.len() > 32 {
        return Err(Error::CircularInheritance("继承层数过深".into()));
    }
    stack.push(id.to_string());
    let child = load(root, id)?;
    let (merged, chain) = if let Some(parent_id) = child.inherits_from.clone() {
        let (parent, mut chain) = resolve_inner(root, &parent_id, stack)?;
        chain.push(child.id.clone());
        (merge(parent, child), chain)
    } else {
        (child.clone(), vec![child.id.clone()])
    };
    stack.pop();
    Ok((merged, chain))
}

fn merge(parent: VersionJson, child: VersionJson) -> VersionJson {
    let mut libraries = child.libraries.clone();
    let mut seen = HashSet::new();
    for lib in &libraries {
        seen.insert(lib.key());
    }
    for lib in parent.libraries {
        if seen.insert(lib.key()) {
            libraries.push(lib);
        }
    }
    let mut extra = parent.extra.clone();
    for (k, v) in child.extra {
        extra.insert(k, v);
    }
    let child_args = child.arguments.clone().unwrap_or_default();
    let parent_args = parent.arguments.clone().unwrap_or_default();
    let mut game = parent_args.game;
    game.extend(child_args.game);
    let mut jvm = parent_args.jvm;
    jvm.extend(child_args.jvm);
    let default_user_jvm = if child_args.default_user_jvm.is_empty() {
        parent_args.default_user_jvm
    } else {
        child_args.default_user_jvm
    };
    VersionJson {
        id: child.id,
        kind: child.kind.or(parent.kind),
        main_class: child.main_class.or(parent.main_class),
        inherits_from: None,
        jar: child.jar.or(parent.jar),
        assets: child.assets.or(parent.assets),
        asset_index: child.asset_index.or(parent.asset_index),
        legacy_arguments: child.legacy_arguments.or(parent.legacy_arguments),
        arguments: Some(Arguments {
            game,
            jvm,
            default_user_jvm,
        }),
        libraries,
        downloads: Downloads {
            client: child.downloads.client.or(parent.downloads.client),
        },
        java_version: child.java_version.or(parent.java_version),
        logging: child.logging.or(parent.logging),
        minimum_launcher_version: std::cmp::max(
            child.minimum_launcher_version,
            parent.minimum_launcher_version,
        ),
        extra,
    }
}

impl Library {
    pub fn key(&self) -> (String, String, String, String) {
        let c = Coord::parse(&self.name).ok();
        match c {
            Some(c) => (c.group, c.artifact, c.classifier.unwrap_or_default(), c.ext),
            None => (
                self.name.clone(),
                String::new(),
                String::new(),
                String::new(),
            ),
        }
    }

    pub fn allowed(&self, ctx: &Context) -> bool {
        allowed(&self.rules, ctx)
    }

    pub fn native_artifact(&self, ctx: &Context) -> Option<(String, Artifact)> {
        if let Some(natives) = &self.natives {
            let template = natives.get(ctx.os.name())?;
            let classifier = template.replace("${arch}", ctx.arch.bits());
            if let Some(dl) = &self.downloads
                && let Some(a) = dl.classifiers.as_ref().and_then(|m| m.get(&classifier))
            {
                return Some((classifier, a.clone()));
            }
            return None;
        }
        let c = Coord::parse(&self.name).ok()?;
        let cl = c.classifier.clone()?;
        if !cl.starts_with("natives-") {
            return None;
        }
        if !classifier_matches(&cl, ctx) {
            return None;
        }
        self.downloads
            .as_ref()
            .and_then(|d| d.artifact.clone())
            .map(|a| (cl, a))
    }

    pub fn is_native(&self, ctx: &Context) -> bool {
        self.native_artifact(ctx).is_some()
    }

    pub fn artifact(&self) -> Option<(String, Artifact)> {
        let dl = self.downloads.as_ref()?;
        let a = dl.artifact.clone()?;
        Some((a.path.clone().unwrap_or_default(), a))
    }
}

pub fn classifier_matches(classifier: &str, ctx: &Context) -> bool {
    let os = ctx.os.name();
    let arch = ctx.arch.name();
    if classifier.contains("natives-windows") {
        if ctx.os != Os::Windows {
            return false;
        }
        if classifier.contains("arm64") {
            return ctx.arch == Arch::Arm64;
        }
        if classifier.contains("x86_64") || classifier.contains("amd64") {
            return ctx.arch == Arch::X86_64;
        }
        return true;
    }
    if classifier.contains("natives-macos") || classifier.contains("natives-osx") {
        if ctx.os != Os::MacOs {
            return false;
        }
        if classifier.contains("arm64") {
            return ctx.arch == Arch::Arm64;
        }
        return true;
    }
    if classifier.contains("natives-linux") {
        if ctx.os != Os::Linux {
            return false;
        }
        if classifier.contains("arm64") {
            return ctx.arch == Arch::Arm64;
        }
        if classifier.contains("x86_64") {
            return ctx.arch == Arch::X86_64;
        }
        return true;
    }
    let _ = (os, arch);
    false
}

pub fn allowed(rules: &[Rule], ctx: &Context) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allow = false;
    for rule in rules {
        if rule_matches(rule, ctx) {
            allow = rule.action == Action::Allow;
        }
    }
    allow
}

fn rule_matches(rule: &Rule, ctx: &Context) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name {
            let name = if name == "osx" { "osx" } else { name.as_str() };
            if name != ctx.os.name() {
                return false;
            }
        }
        if let Some(arch) = &os.arch
            && normalize_arch(arch) != ctx.arch.name()
        {
            return false;
        }
        if os.version.is_some() || os.version_range.is_some() {
            let Some(version) = &ctx.os_version else {
                return false;
            };
            if let Some(v) = &os.version
                && !version_matches_exact(version, v)
            {
                return false;
            }
            if let Some(r) = &os.version_range {
                if let Some(min) = &r.min
                    && compare_versions(version, min) < 0
                {
                    return false;
                }
                if let Some(max) = &r.max
                    && compare_versions(version, max) > 0
                {
                    return false;
                }
            }
        }
    }
    if let Some(features) = &rule.features {
        for (k, v) in features {
            let actual = match k.as_str() {
                "is_demo_user" => ctx.features.demo,
                "has_custom_resolution" => ctx.features.custom_resolution,
                _ => false,
            };
            if actual != *v {
                return false;
            }
        }
    }
    true
}

fn normalize_arch(arch: &str) -> &'static str {
    match arch {
        "x86" | "i386" | "i686" => "x86",
        "arm64" | "aarch64" => "arm64",
        _ => "x86_64",
    }
}

fn version_matches_exact(actual: &str, expected: &str) -> bool {
    actual == expected || actual.starts_with(expected)
}

fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |s: &str| -> Vec<i64> {
        s.split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let aa = parse(a);
    let bb = parse(b);
    for i in 0..aa.len().max(bb.len()) {
        let x = aa.get(i).copied().unwrap_or(0);
        let y = bb.get(i).copied().unwrap_or(0);
        if x < y {
            return -1;
        }
        if x > y {
            return 1;
        }
    }
    0
}

#[derive(Debug, Clone)]
pub struct Coord {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub ext: String,
}

impl Coord {
    pub fn parse(name: &str) -> Result<Self> {
        let (body, ext) = match name.split_once('@') {
            Some((b, e)) => (b, e.to_string()),
            None => (name, "jar".to_string()),
        };
        let parts: Vec<&str> = body.split(':').collect();
        if parts.len() < 3 {
            return Err(Error::Unsupported(format!("无效的库坐标: {name}")));
        }
        Ok(Self {
            group: parts[0].to_string(),
            artifact: parts[1].to_string(),
            version: parts[2].to_string(),
            classifier: parts.get(3).map(|s| s.to_string()),
            ext,
        })
    }

    pub fn rel_path(&self) -> PathBuf {
        let mut p = PathBuf::new();
        for part in self.group.split('.') {
            p.push(part);
        }
        p.push(&self.artifact);
        p.push(&self.version);
        let mut file = format!("{}-{}", self.artifact, self.version);
        if let Some(c) = &self.classifier {
            file.push('-');
            file.push_str(c);
        }
        file.push('.');
        file.push_str(&self.ext);
        p.push(file);
        p
    }

    pub fn url(&self, base: &str) -> String {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            self.rel_path().to_string_lossy().replace('\\', "/")
        )
    }
}

pub fn asset_index(resolved: &Resolved) -> Result<AssetIndex> {
    resolved
        .asset_index
        .clone()
        .ok_or_else(|| Error::Unsupported("该版本没有 assetIndex（旧版资源暂不支持）".into()))
}

pub fn client_download(root: &Root, resolved: &Resolved) -> Result<crate::http::FileSpec> {
    // 从叶子向根找 downloads.client
    let mut id = resolved
        .chain
        .last()
        .cloned()
        .unwrap_or_else(|| resolved.id.clone());
    // chain[0] 是 vanilla，通常有 client
    if let Some(first) = resolved.chain.first() {
        id = first.clone();
    }
    let meta = load(root, &id)?;
    let d = meta
        .downloads
        .client
        .ok_or_else(|| Error::Unsupported("版本没有 client 下载信息".into()))?;
    Ok(crate::http::FileSpec {
        url: d.url,
        dest: root.client_jar(&id),
        sha1: d.sha1,
        size: d.size,
        verify_existing: true,
    })
}

pub fn library_specs(
    root: &Root,
    resolved: &Resolved,
    ctx: &Context,
) -> Vec<crate::http::FileSpec> {
    library_specs_for(root, &resolved.libraries, ctx)
}

pub fn library_specs_for(
    root: &Root,
    libraries: &[Library],
    ctx: &Context,
) -> Vec<crate::http::FileSpec> {
    let mut out = Vec::new();
    for lib in libraries {
        if !lib.allowed(ctx) {
            continue;
        }
        if let Some((_cl, a)) = lib.native_artifact(ctx) {
            out.push(spec_from_artifact(root, lib, &a));
            continue;
        }
        if let Some((_path, a)) = lib.artifact() {
            out.push(spec_from_artifact(root, lib, &a));
            continue;
        }
        if let Ok(c) = Coord::parse(&lib.name) {
            let base = lib
                .url
                .clone()
                .unwrap_or_else(|| "https://libraries.minecraft.net/".into());
            let sha1 = lib
                .extra
                .get("sha1")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            out.push(crate::http::FileSpec {
                url: c.url(&base),
                dest: root.libraries().join(c.rel_path()),
                sha1,
                size: None,
                verify_existing: false,
            });
        }
    }
    out
}

fn spec_from_artifact(root: &Root, lib: &Library, a: &Artifact) -> crate::http::FileSpec {
    let path = a.path.clone().unwrap_or_else(|| {
        Coord::parse(&lib.name)
            .map(|c| c.rel_path().to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    });
    let url = if a.url.is_empty() {
        // Processor outputs (Forge/NeoForge patched client) have no URL;
        // the processor engine creates the file locally.
        String::new()
    } else if a.url.starts_with("http") {
        a.url.clone()
    } else {
        let base = lib
            .url
            .clone()
            .unwrap_or_else(|| "https://libraries.minecraft.net/".into());
        Coord::parse(&lib.name)
            .map(|c| c.url(&base))
            .unwrap_or_else(|_| a.url.clone())
    };
    crate::http::FileSpec {
        url,
        dest: root.libraries().join(path),
        sha1: a.sha1.clone(),
        size: a.size,
        verify_existing: true,
    }
}

pub fn classpath(root: &Root, resolved: &Resolved, ctx: &Context) -> Vec<PathBuf> {
    let mut cp = Vec::new();
    for lib in &resolved.libraries {
        if !lib.allowed(ctx) || lib.is_native(ctx) {
            continue;
        }
        if let Some((path, _)) = lib.artifact() {
            let p = if path.is_empty() {
                Coord::parse(&lib.name)
                    .map(|c| root.libraries().join(c.rel_path()))
                    .unwrap_or_default()
            } else {
                root.libraries().join(path)
            };
            if p.exists() {
                cp.push(p);
            }
        } else if let Ok(c) = Coord::parse(&lib.name) {
            let p = root.libraries().join(c.rel_path());
            if p.exists() {
                cp.push(p);
            }
        }
    }
    cp.push(resolved.client_jar.clone());
    cp
}

pub fn native_jars(root: &Root, resolved: &Resolved, ctx: &Context) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for lib in &resolved.libraries {
        if !lib.allowed(ctx) {
            continue;
        }
        if let Some((_cl, a)) = lib.native_artifact(ctx) {
            let path = a.path.clone().unwrap_or_default();
            let p = root.libraries().join(path);
            if p.exists() {
                out.push(p);
            }
        }
    }
    out
}

pub fn read_json(path: &Path) -> Result<Value> {
    let data = std::fs::read(path).map_err(|e| crate::error::io(path, e))?;
    serde_json::from_slice(&data).map_err(|e| json(path, e))
}

pub fn load_map(_root: &Root) -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux() -> Context {
        Context {
            os: Os::Linux,
            arch: Arch::X86_64,
            os_version: Some("6.8.0".into()),
            features: Features::default(),
        }
    }

    #[test]
    fn coord_parses_classifier_and_extension() {
        let c = Coord::parse("org.lwjgl:lwjgl:3.3.3:natives-linux").unwrap();
        assert_eq!(c.classifier.as_deref(), Some("natives-linux"));
        assert_eq!(
            c.rel_path().to_string_lossy().replace('\\', "/"),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar"
        );

        let c = Coord::parse("net.neoforged:neoform:1.21.1:data@zip").unwrap();
        assert_eq!(c.ext, "zip");
        assert!(
            c.rel_path()
                .to_string_lossy()
                .ends_with("neoform-1.21.1-data.zip")
        );
    }

    #[test]
    fn rules_respect_os_and_arch() {
        let windows_only = vec![Rule {
            action: Action::Allow,
            os: Some(OsRule {
                name: Some("windows".into()),
                ..Default::default()
            }),
            features: None,
        }];
        assert!(!allowed(&windows_only, &linux()));

        let linux_x64 = vec![Rule {
            action: Action::Allow,
            os: Some(OsRule {
                name: Some("linux".into()),
                arch: Some("x86_64".into()),
                ..Default::default()
            }),
            features: None,
        }];
        assert!(allowed(&linux_x64, &linux()));

        let no_rules: Vec<Rule> = Vec::new();
        assert!(allowed(&no_rules, &linux()));
    }

    #[test]
    fn rules_support_version_range_and_features() {
        let ctx = Context {
            os: Os::Windows,
            arch: Arch::X86_64,
            os_version: Some("10.0.22631".into()),
            features: Features::default(),
        };
        let zgc = vec![Rule {
            action: Action::Allow,
            os: Some(OsRule {
                name: Some("windows".into()),
                version_range: Some(VersionRange {
                    min: Some("10.0.17134".into()),
                    max: None,
                }),
                ..Default::default()
            }),
            features: None,
        }];
        assert!(allowed(&zgc, &ctx));

        let demo = vec![Rule {
            action: Action::Allow,
            os: None,
            features: Some(BTreeMap::from([("is_demo_user".to_string(), true)])),
        }];
        assert!(!allowed(&demo, &ctx));
        let mut ctx2 = ctx.clone();
        ctx2.features.demo = true;
        assert!(allowed(&demo, &ctx2));
    }

    #[test]
    fn native_classifier_matches_platform() {
        let lib = Library {
            name: "org.lwjgl:lwjgl:3.3.3:natives-linux".into(),
            downloads: Some(LibDownloads {
                artifact: Some(Artifact {
                    path: Some("x".into()),
                    sha1: None,
                    size: None,
                    url: "u".into(),
                }),
                classifiers: None,
            }),
            natives: None,
            rules: vec![],
            extract: None,
            url: None,
            extra: Default::default(),
        };
        assert!(lib.is_native(&linux()));
        assert!(lib.native_artifact(&linux()).is_some());
    }
}
