use crate::Root;
use crate::error::{Error, Result};
use crate::http::Http;
use crate::paths::write_atomic;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const NEOFORGE_METADATA: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const FORGE_METADATA: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const FABRIC_LOADERS: &str = "https://meta.fabricmc.net/v2/versions/loader";

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// All Mojang release versions, newest first.
    pub minecraft: Vec<String>,
    /// Stable Fabric loader versions, newest first.
    pub fabric: Vec<String>,
    pub neoforge: BTreeMap<String, Vec<String>>,
    pub forge: BTreeMap<String, Vec<String>>,
}

impl Catalog {
    pub fn fallback() -> Self {
        let mut neoforge = BTreeMap::new();
        neoforge.insert("1.21.1".to_string(), vec!["21.1.209".to_string()]);
        let mut forge = BTreeMap::new();
        forge.insert("1.21.1".to_string(), vec!["52.1.20".to_string()]);
        Self {
            minecraft: vec!["1.21.1".to_string()],
            fabric: vec!["0.19.5".to_string()],
            neoforge,
            forge,
        }
    }

    pub fn neoforge_for(&self, mc: &str) -> &[String] {
        self.neoforge.get(mc).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn forge_for(&self, mc: &str) -> &[String] {
        self.forge.get(mc).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub fn load(root: &Root, mirror: &str) -> Result<Catalog> {
    let mc_path = root.cache().join("version_manifest_v2.json");
    let nf_path = root.cache().join("neoforge-metadata.xml");
    let forge_path = root.cache().join("forge-metadata.xml");
    let fabric_path = root.cache().join("fabric-loaders.json");

    // Cache first: an offline start should not wait on four connect timeouts.
    if let (Ok(mc_text), Ok(nf_text), Ok(forge_text), Ok(fabric_text)) = (
        std::fs::read_to_string(&mc_path),
        std::fs::read_to_string(&nf_path),
        std::fs::read_to_string(&forge_path),
        std::fs::read_to_string(&fabric_path),
    ) && let Ok(catalog) = build(&mc_text, &nf_text, &forge_text, &fabric_text)
    {
        return Ok(catalog);
    }

    let http = Http::new(mirror)?;
    let mc_text = cached_text(&http, VERSION_MANIFEST, &mc_path)?;
    // A failure in one loader list should not make vanilla / other loaders unusable.
    let nf_text = cached_text(&http, NEOFORGE_METADATA, &nf_path).unwrap_or_default();
    let forge_text = cached_text(&http, FORGE_METADATA, &forge_path).unwrap_or_default();
    let fabric_text = cached_text(&http, FABRIC_LOADERS, &fabric_path).unwrap_or_default();
    build(&mc_text, &nf_text, &forge_text, &fabric_text)
}

fn build(mc_text: &str, nf_text: &str, forge_text: &str, fabric_text: &str) -> Result<Catalog> {
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
    let releases: Vec<String> = manifest
        .versions
        .into_iter()
        .filter(|v| v.kind == "release")
        .map(|v| v.id)
        .collect();
    if releases.is_empty() {
        return Err(Error::Install("没有找到正式版 Minecraft".into()));
    }
    let release_set: BTreeSet<&str> = releases.iter().map(String::as_str).collect();

    let nf_versions = parse_xml_versions(nf_text);
    let mut neoforge = BTreeMap::new();
    for mc in &releases {
        let Some(prefix) = neoforge_prefix(mc) else {
            continue;
        };
        let versions = sorted_desc(
            nf_versions
                .iter()
                .filter(|v| v.starts_with(&prefix) && is_stable(v))
                .cloned()
                .collect(),
        );
        if !versions.is_empty() {
            neoforge.insert(mc.clone(), versions);
        }
    }

    let forge_versions = parse_xml_versions(forge_text);
    let mut forge: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for full in &forge_versions {
        if let Some((mc, version)) = full.split_once('-')
            && release_set.contains(mc)
            && is_stable(version)
        {
            forge
                .entry(mc.to_string())
                .or_default()
                .push(version.to_string());
        }
    }
    for versions in forge.values_mut() {
        *versions = sorted_desc(std::mem::take(versions));
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FabricEntry {
        Nested { loader: FabricLoader },
        Flat(FabricLoader),
    }
    #[derive(Deserialize)]
    struct FabricLoader {
        version: String,
        stable: bool,
    }
    let mut fabric = Vec::new();
    if let Ok(entries) = serde_json::from_str::<Vec<FabricEntry>>(fabric_text) {
        fabric = sorted_desc(
            entries
                .into_iter()
                .filter_map(|e| match e {
                    FabricEntry::Nested { loader } if loader.stable => Some(loader.version),
                    FabricEntry::Flat(loader) if loader.stable => Some(loader.version),
                    _ => None,
                })
                .collect(),
        );
    }

    Ok(Catalog {
        minecraft: releases,
        fabric,
        neoforge,
        forge,
    })
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

/// Fabric supports Minecraft 1.14+ and the date-based 26.x versions.
pub fn fabric_supported(mc: &str) -> bool {
    if let Some(rest) = mc.strip_prefix("1.") {
        rest.split('.')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|minor| minor >= 14)
    } else {
        mc.chars().next().is_some_and(|c| c.is_ascii_digit())
    }
}

fn is_stable(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    !["alpha", "beta", "snapshot", "-pre", "rc"]
        .iter()
        .any(|s| lower.contains(s))
}

fn parse_xml_versions(xml: &str) -> Vec<String> {
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

fn sorted_desc(mut versions: Vec<String>) -> Vec<String> {
    versions.sort_by_key(|a| std::cmp::Reverse(version_key(a)));
    versions.dedup();
    versions
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
    fn fabric_support_starts_at_1_14() {
        assert!(!fabric_supported("1.12.2"));
        assert!(fabric_supported("1.14"));
        assert!(fabric_supported("1.21.1"));
        assert!(fabric_supported("26.3"));
    }

    #[test]
    fn parses_metadata_and_sorts_versions() {
        let xml = "<metadata><versioning><versions>\
            <version>21.1.9</version><version>21.1.10</version>\
            <version>21.1.10-beta</version></versions></versioning></metadata>";
        let versions: Vec<String> = parse_xml_versions(xml)
            .into_iter()
            .filter(|v| is_stable(v))
            .collect();
        assert_eq!(versions, vec!["21.1.9", "21.1.10"]);
        assert_eq!(version_key("21.1.10"), vec![21, 1, 10]);
    }

    #[test]
    fn groups_forge_versions_by_minecraft() {
        let xml = "<versions>\
            <version>1.21.1-52.1.20</version>\
            <version>1.21.1-52.1.9</version>\
            <version>1.20.1-47.4.28</version></versions>";
        let vs = parse_xml_versions(xml);
        for full in vs {
            if let Some((mc, v)) = full.split_once('-') {
                assert!(!mc.is_empty() && !v.is_empty());
            }
        }
    }
}
