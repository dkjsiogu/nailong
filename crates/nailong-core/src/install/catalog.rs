use crate::Root;
use crate::error::{Error, Result};
use crate::http::Http;
use crate::paths::write_atomic;
use serde::Deserialize;
use std::collections::BTreeMap;

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const NEOFORGE_METADATA: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Release versions that have at least one stable NeoForge build, newest first.
    pub minecraft: Vec<String>,
    pub neoforge: BTreeMap<String, Vec<String>>,
}

impl Catalog {
    pub fn fallback() -> Self {
        let mut neoforge = BTreeMap::new();
        neoforge.insert("1.21.1".to_string(), vec!["21.1.209".to_string()]);
        Self {
            minecraft: vec!["1.21.1".to_string()],
            neoforge,
        }
    }

    pub fn neoforge_for(&self, mc: &str) -> &[String] {
        self.neoforge.get(mc).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub fn load(root: &Root, mirror: &str) -> Result<Catalog> {
    let mc_path = root.cache().join("version_manifest_v2.json");
    let nf_path = root.cache().join("neoforge-metadata.xml");

    // Cache first: version lists change rarely, and an offline start should
    // not wait on three connect timeouts before showing the install screen.
    if let (Ok(mc_text), Ok(nf_text)) = (
        std::fs::read_to_string(&mc_path),
        std::fs::read_to_string(&nf_path),
    ) && let Ok(catalog) = build(&mc_text, &nf_text)
    {
        return Ok(catalog);
    }

    let http = Http::new(mirror)?;
    let mc_text = cached_text(&http, VERSION_MANIFEST, &mc_path)?;
    let nf_text = cached_text(&http, NEOFORGE_METADATA, &nf_path)?;
    build(&mc_text, &nf_text)
}

fn build(mc_text: &str, nf_text: &str) -> Result<Catalog> {
    #[derive(Deserialize)]
    struct Manifest {
        versions: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(rename = "type")]
        kind: String,
    }

    let manifest: Manifest = serde_json::from_str(mc_text)
        .map_err(|e| Error::Install(format!("版本清单格式异常: {e}")))?;
    let nf_versions = parse_neoforge_versions(nf_text);
    if nf_versions.is_empty() {
        return Err(Error::Install("NeoForge 版本列表为空".into()));
    }

    let mut catalog = Catalog::default();
    for entry in manifest.versions {
        if entry.kind != "release" {
            continue;
        }
        let Some(prefix) = neoforge_prefix(&entry.id) else {
            continue;
        };
        let versions: Vec<String> = nf_versions
            .iter()
            .filter(|v| v.starts_with(&prefix) && is_stable(v))
            .cloned()
            .collect();
        if !versions.is_empty() {
            let mut versions = versions;
            versions.sort_by_key(|a| std::cmp::Reverse(version_key(a)));
            catalog.neoforge.insert(entry.id.clone(), versions);
            catalog.minecraft.push(entry.id);
        }
    }
    if catalog.minecraft.is_empty() {
        return Err(Error::Install("没有找到可用的 NeoForge 版本组合".into()));
    }
    Ok(catalog)
}

fn cached_text(http: &Http, url: &str, cache: &std::path::Path) -> Result<String> {
    match http.get_text(url) {
        Ok(text) => {
            let _ = write_atomic(cache, text.as_bytes());
            Ok(text)
        }
        Err(e) => std::fs::read_to_string(cache).map_err(|_| e),
    }
}

/// NeoForge versions use the Minecraft version without the leading `1.`:
/// 1.21.1 -> 21.1.x, 1.21 -> 21.0.x, 26.3 -> 26.3.0.x.
pub fn neoforge_prefix(mc: &str) -> Option<String> {
    if mc == "1.20.1" {
        // The 1.20.1 line uses the old net.neoforged:forge coordinates.
        return None;
    }
    let parts: Vec<&str> = mc.split('.').collect();
    if parts.first() == Some(&"1") {
        return match parts.len() {
            2 => Some(format!("{}.0.", parts[1])),
            3.. => Some(format!("{}.{}.", parts[1], parts[2])),
            _ => None,
        };
    }
    if mc.chars().next()?.is_ascii_digit() {
        if parts.len() == 2 {
            Some(format!("{mc}.0."))
        } else {
            Some(format!("{mc}."))
        }
    } else {
        None
    }
}

fn is_stable(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    !["alpha", "beta", "snapshot", "-pre", "rc"]
        .iter()
        .any(|s| lower.contains(s))
}

fn parse_neoforge_versions(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        rest = &rest[start + "<version>".len()..];
        let Some(end) = rest.find("</version>") else {
            break;
        };
        let v = rest[..end].trim();
        if !v.is_empty() {
            out.push(v.to_string());
        }
        rest = &rest[end + "</version>".len()..];
    }
    out
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_minecraft_to_neoforge_prefix() {
        assert_eq!(neoforge_prefix("1.21.1").as_deref(), Some("21.1."));
        assert_eq!(neoforge_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(neoforge_prefix("26.3").as_deref(), Some("26.3.0."));
        assert_eq!(neoforge_prefix("26.3.1").as_deref(), Some("26.3.1."));
        assert_eq!(neoforge_prefix("1.20.1"), None);
    }

    #[test]
    fn parses_metadata_and_sorts_versions() {
        let xml = "<metadata><versioning><versions>\
            <version>21.1.9</version><version>21.1.10</version>\
            <version>21.1.10-beta</version></versions></versioning></metadata>";
        let versions: Vec<String> = parse_neoforge_versions(xml)
            .into_iter()
            .filter(|v| is_stable(v))
            .collect();
        assert_eq!(versions, vec!["21.1.9", "21.1.10"]);
        assert_eq!(version_key("21.1.10"), vec![21, 1, 10]);
    }
}
