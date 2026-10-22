//! Modrinth / CurseForge mod search, version selection and dependency plans.
//!
//! The GUI locks these queries to one installed version's Minecraft version
//! and loader. Downloads are ordinary files under that instance's `mods/`
//! directory; no modpack format or loader metadata is rewritten.

use crate::builtin;
use crate::cancel;
use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::progress::{Progress, Sink};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Component, Path};

const MODRINTH_API: &str = "https://api.modrinth.com/v2";
const CURSEFORGE_API: &str = "https://api.curseforge.com/v1";
const CURSEFORGE_GAME_ID: u64 = 432;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Source {
    Modrinth,
    CurseForge,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::Modrinth => "Modrinth",
            Self::CurseForge => "CurseForge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Loader {
    Fabric,
    Forge,
    NeoForge,
    Quilt,
}

impl Loader {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
            Self::Quilt => "Quilt",
        }
    }

    fn modrinth_category(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::Quilt => "quilt",
        }
    }

    fn curseforge_id(self) -> u16 {
        match self {
            Self::Forge => 1,
            Self::Fabric => 4,
            Self::Quilt => 5,
            Self::NeoForge => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContentKind {
    Mod,
    ResourcePack,
    Shader,
}

impl ContentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mod => "Mod",
            Self::ResourcePack => "资源包",
            Self::Shader => "光影",
        }
    }

    pub fn folder(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::ResourcePack => "resourcepacks",
            Self::Shader => "shaderpacks",
        }
    }

    fn modrinth_type(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resourcepack",
            Self::Shader => "shader",
        }
    }

    fn curseforge_class(self) -> u64 {
        match self {
            Self::Mod => 6,
            Self::ResourcePack => 12,
            Self::Shader => 6552,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
    Tool,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub source: Source,
    pub kind: ContentKind,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub author: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub source: Source,
    pub project_id: String,
    pub version_id: Option<String>,
    pub name: Option<String>,
    pub kind: DependencyKind,
}

#[derive(Debug, Clone)]
pub struct Version {
    pub source: Source,
    pub kind: ContentKind,
    pub project_id: String,
    pub id: String,
    pub name: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub file_name: String,
    pub download_url: Option<String>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub published: Option<String>,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone)]
pub struct SearchPage {
    pub hits: Vec<Project>,
    pub total: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DependencyChoice {
    pub dependency: Dependency,
    pub project: Option<Project>,
    pub version: Option<Version>,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadReport {
    pub downloaded: usize,
    pub skipped: Vec<String>,
}

fn query_url(base: &str, params: &[(&str, String)]) -> Result<String> {
    let mut url =
        reqwest::Url::parse(base).map_err(|e| Error::Install(format!("Mod API URL 无效: {e}")))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.into())
}

fn curse_key() -> Result<String> {
    builtin::curseforge_api_key()
        .ok_or_else(|| Error::Install("此构建没有 CurseForge API key".into()))
}

fn curse_headers(key: &str) -> [(&str, &str); 2] {
    [("x-api-key", key), ("Accept", "application/json")]
}

pub fn search(
    query: &str,
    source: Option<Source>,
    mc: &str,
    loader: Option<Loader>,
    kind: ContentKind,
    offset: usize,
    limit: usize,
) -> Result<SearchPage> {
    cancel::check()?;
    let limit = limit.clamp(1, 50);
    let mut page = SearchPage {
        hits: Vec::new(),
        total: 0,
        warnings: Vec::new(),
    };
    let mut last_error = None;
    let want = |candidate: Source| source.is_none_or(|wanted| wanted == candidate);

    if want(Source::Modrinth) {
        match search_modrinth(query, mc, loader, kind, offset, limit) {
            Ok(mut result) => {
                page.hits.append(&mut result.hits);
                page.total = page.total.max(result.total);
            }
            Err(e) => {
                page.warnings.push(format!("Modrinth: {e}"));
                last_error = Some(e);
            }
        }
    }
    if want(Source::CurseForge) {
        match search_curseforge(query, mc, loader, kind, offset, limit) {
            Ok(mut result) => {
                page.hits.append(&mut result.hits);
                page.total = page.total.max(result.total);
            }
            Err(e) => {
                page.warnings.push(format!("CurseForge: {e}"));
                last_error = Some(e);
            }
        }
    }
    if page.hits.is_empty()
        && let Some(error) = last_error
    {
        return Err(error);
    }
    page.hits
        .sort_by_key(|project| std::cmp::Reverse(project.downloads));
    Ok(page)
}

fn search_modrinth(
    query: &str,
    mc: &str,
    loader: Option<Loader>,
    kind: ContentKind,
    offset: usize,
    limit: usize,
) -> Result<SearchPage> {
    let mut facets = vec![
        vec![format!("project_type:{}", kind.modrinth_type())],
        vec![format!("versions:{mc}")],
    ];
    if kind == ContentKind::Mod {
        facets.push(vec![format!(
            "categories:{}",
            loader.unwrap_or(Loader::Fabric).modrinth_category()
        )]);
    }
    let facets = serde_json::to_string(&facets)
        .map_err(|e| Error::Install(format!("Modrinth facets 序列化失败: {e}")))?;
    let url = query_url(
        &format!("{MODRINTH_API}/search"),
        &[
            ("query", query.to_string()),
            ("facets", facets),
            ("offset", offset.to_string()),
            ("limit", limit.to_string()),
        ],
    )?;
    let value = Http::new("official")?.get_json(&url)?;
    let hits = value
        .get("hits")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|value| modrinth_project(value, kind))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(SearchPage {
        hits,
        total: value
            .get("total_hits")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        warnings: Vec::new(),
    })
}

fn search_curseforge(
    query: &str,
    mc: &str,
    loader: Option<Loader>,
    kind: ContentKind,
    offset: usize,
    limit: usize,
) -> Result<SearchPage> {
    let key = curse_key()?;
    let mut params = vec![
        ("gameId", CURSEFORGE_GAME_ID.to_string()),
        ("classId", kind.curseforge_class().to_string()),
        ("gameVersion", mc.to_string()),
        ("index", offset.to_string()),
        ("pageSize", limit.min(50).to_string()),
        ("sortOrder", "desc".to_string()),
    ];
    if kind == ContentKind::Mod
        && let Some(loader) = loader
    {
        params.push(("modLoaderType", loader.curseforge_id().to_string()));
    }
    if !query.trim().is_empty() {
        params.push(("searchFilter", query.trim().to_string()));
    }
    let url = query_url(&format!("{CURSEFORGE_API}/mods/search"), &params)?;
    let value = Http::new("official")?.get_json_with_headers(&url, &curse_headers(&key))?;
    let hits = value
        .get("data")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|value| curseforge_project(value, kind))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(SearchPage {
        hits,
        total: value
            .pointer("/pagination/totalCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        warnings: Vec::new(),
    })
}

fn modrinth_project(value: &Value, kind: ContentKind) -> Option<Project> {
    Some(Project {
        source: Source::Modrinth,
        kind,
        id: value
            .get("project_id")
            .or_else(|| value.get("id"))?
            .as_str()?
            .to_string(),
        slug: value
            .get("slug")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: value.get("title")?.as_str()?.to_string(),
        summary: value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        author: value
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        downloads: value.get("downloads").and_then(|v| v.as_u64()).unwrap_or(0),
        icon_url: value
            .get("icon_url")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        categories: string_array(value.get("categories")),
    })
}

fn curseforge_project(value: &Value, kind: ContentKind) -> Option<Project> {
    let id = value.get("id")?.as_u64()?.to_string();
    Some(Project {
        source: Source::CurseForge,
        kind,
        id,
        slug: value
            .get("slug")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: value.get("name")?.as_str()?.to_string(),
        summary: value
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        author: value
            .pointer("/authors/0/name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        downloads: value
            .get("downloadCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        icon_url: value
            .pointer("/logo/thumbnailUrl")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        categories: value
            .get("categories")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn project(source: Source, id: &str, kind: ContentKind) -> Result<Project> {
    cancel::check()?;
    let value = match source {
        Source::Modrinth => {
            let url = format!("{MODRINTH_API}/project/{id}");
            Http::new("official")?.get_json(&url)?
        }
        Source::CurseForge => {
            let key = curse_key()?;
            let url = format!("{CURSEFORGE_API}/mods/{id}");
            Http::new("official")?.get_json_with_headers(&url, &curse_headers(&key))?
        }
    };
    let data = if source == Source::CurseForge {
        value.get("data").unwrap_or(&value)
    } else {
        &value
    };
    match source {
        Source::Modrinth => modrinth_project(data, kind),
        Source::CurseForge => curseforge_project(data, kind),
    }
    .ok_or_else(|| Error::Install(format!("{} 项目 {id} 解析失败", source.label())))
}

pub fn versions(project: &Project, mc: &str, loader: Loader) -> Result<Vec<Version>> {
    cancel::check()?;
    let mut versions = match project.source {
        Source::Modrinth => versions_modrinth(&project.id, project.kind, mc, loader)?,
        Source::CurseForge => versions_curseforge(&project.id, project.kind, mc, loader)?,
    };
    versions.sort_by(|a, b| b.published.cmp(&a.published));
    Ok(versions)
}

fn versions_modrinth(
    project_id: &str,
    kind: ContentKind,
    mc: &str,
    loader: Loader,
) -> Result<Vec<Version>> {
    let mut params = vec![("game_versions", serde_json::json!([mc]).to_string())];
    if kind == ContentKind::Mod {
        params.push((
            "loaders",
            serde_json::json!([loader.modrinth_category()]).to_string(),
        ));
    }
    let url = query_url(
        &format!("{MODRINTH_API}/project/{project_id}/version"),
        &params,
    )?;
    let value = Http::new("official")?.get_json(&url)?;
    let items = value
        .as_array()
        .ok_or_else(|| Error::Install("Modrinth 版本列表格式异常".into()))?;
    Ok(items
        .iter()
        .filter_map(|item| modrinth_version(project_id, kind, item))
        .collect())
}

fn modrinth_version(project_id: &str, kind: ContentKind, value: &Value) -> Option<Version> {
    let files = value.get("files")?.as_array()?;
    let file = files
        .iter()
        .find(|file| {
            file.get("primary")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .or_else(|| files.first())?;
    let dependencies = value
        .get("dependencies")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(modrinth_dependency).collect())
        .unwrap_or_default();
    Some(Version {
        source: Source::Modrinth,
        kind,
        project_id: project_id.to_string(),
        id: value.get("id")?.as_str()?.to_string(),
        name: value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        version_number: value
            .get("version_number")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        game_versions: string_array(value.get("game_versions")),
        loaders: string_array(value.get("loaders")),
        file_name: file.get("filename")?.as_str()?.to_string(),
        download_url: file.get("url").and_then(|v| v.as_str()).map(str::to_string),
        sha1: file
            .pointer("/hashes/sha1")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        size: file.get("size").and_then(|v| v.as_u64()),
        published: value
            .get("date_published")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        dependencies,
    })
}

fn modrinth_dependency(value: &Value) -> Option<Dependency> {
    let project_id = value.get("project_id")?.as_str()?.to_string();
    let kind = match value
        .get("dependency_type")
        .and_then(|v| v.as_str())
        .unwrap_or("required")
    {
        "optional" => DependencyKind::Optional,
        "incompatible" => DependencyKind::Incompatible,
        "embedded" => DependencyKind::Embedded,
        _ => DependencyKind::Required,
    };
    Some(Dependency {
        source: Source::Modrinth,
        project_id,
        version_id: value
            .get("version_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        name: value
            .get("file_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        kind,
    })
}

fn versions_curseforge(
    project_id: &str,
    kind: ContentKind,
    mc: &str,
    loader: Loader,
) -> Result<Vec<Version>> {
    let key = curse_key()?;
    let mut params = vec![
        ("gameVersion", mc.to_string()),
        ("pageSize", "50".to_string()),
    ];
    if kind == ContentKind::Mod {
        params.push(("modLoaderType", loader.curseforge_id().to_string()));
    }
    let url = query_url(
        &format!("{CURSEFORGE_API}/mods/{project_id}/files"),
        &params,
    )?;
    let value = Http::new("official")?.get_json_with_headers(&url, &curse_headers(&key))?;
    let items = value
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Install("CurseForge 文件列表格式异常".into()))?;
    Ok(items
        .iter()
        .filter(|file| file.get("isAvailable").and_then(|v| v.as_bool()) != Some(false))
        .filter_map(|file| curseforge_version(project_id, kind, file))
        .collect())
}

fn curseforge_version(project_id: &str, kind: ContentKind, value: &Value) -> Option<Version> {
    let file_name = value.get("fileName")?.as_str()?.to_string();
    let file_id = value.get("id")?.as_u64()?;
    let dependencies = value
        .get("dependencies")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let kind = match item.get("relationType").and_then(|v| v.as_u64()) {
                        Some(2) => DependencyKind::Optional,
                        Some(3) => DependencyKind::Required,
                        Some(5) => DependencyKind::Incompatible,
                        Some(1) | Some(4) | Some(6) => return None,
                        _ => return None,
                    };
                    Some(Dependency {
                        source: Source::CurseForge,
                        project_id: item.get("modId")?.as_u64()?.to_string(),
                        version_id: None,
                        name: None,
                        kind,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Version {
        source: Source::CurseForge,
        kind,
        project_id: project_id.to_string(),
        id: file_id.to_string(),
        name: value
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or(&file_name)
            .to_string(),
        version_number: value
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or(&file_name)
            .to_string(),
        game_versions: string_array(value.get("gameVersions")),
        loaders: Vec::new(),
        file_name,
        download_url: value
            .get("downloadUrl")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                Some(format!(
                    "https://www.curseforge.com/api/v1/mods/{project_id}/files/{file_id}/download"
                ))
            }),
        sha1: value
            .get("hashes")
            .and_then(|v| v.as_array())
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("algo").and_then(|v| v.as_u64()) == Some(1))
                    .and_then(|item| item.get("value").and_then(|v| v.as_str()))
                    .map(str::to_string)
            }),
        size: value.get("fileLength").and_then(|v| v.as_u64()),
        published: value
            .get("fileDate")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        dependencies,
    })
}

fn version_from_id(
    source: Source,
    project_id: &str,
    version_id: &str,
    kind: ContentKind,
) -> Result<Option<Version>> {
    match source {
        Source::Modrinth => {
            let url = format!("{MODRINTH_API}/version/{version_id}");
            let value = Http::new("official")?.get_json(&url)?;
            Ok(modrinth_version(project_id, kind, &value))
        }
        Source::CurseForge => {
            let key = curse_key()?;
            let url = format!("{CURSEFORGE_API}/mods/{project_id}/files/{version_id}");
            let value = Http::new("official")?.get_json_with_headers(&url, &curse_headers(&key))?;
            let data = value.get("data").unwrap_or(&value);
            Ok(curseforge_version(project_id, kind, data))
        }
    }
}

fn resolve_dependency(
    dependency: &Dependency,
    mc: &str,
    loader: Loader,
) -> Result<Option<(Option<Project>, Option<Version>)>> {
    cancel::check()?;
    let project = project(dependency.source, &dependency.project_id, ContentKind::Mod).ok();
    let version = if let Some(version_id) = &dependency.version_id {
        version_from_id(
            dependency.source,
            &dependency.project_id,
            version_id,
            ContentKind::Mod,
        )?
    } else {
        let project_ref = project.clone().unwrap_or(Project {
            source: dependency.source,
            kind: ContentKind::Mod,
            id: dependency.project_id.clone(),
            slug: String::new(),
            name: dependency.name.clone().unwrap_or_default(),
            summary: String::new(),
            author: String::new(),
            downloads: 0,
            icon_url: None,
            categories: Vec::new(),
        });
        versions(&project_ref, mc, loader)?.into_iter().next()
    };
    Ok(Some((project, version)))
}

pub fn dependency_choices(
    version: &Version,
    mc: &str,
    loader: Loader,
) -> Result<Vec<DependencyChoice>> {
    let mut choices = Vec::new();
    for dependency in &version.dependencies {
        if matches!(
            dependency.kind,
            DependencyKind::Embedded | DependencyKind::Tool
        ) {
            continue;
        }
        let (project, resolved) =
            resolve_dependency(dependency, mc, loader)?.unwrap_or((None, None));
        choices.push(DependencyChoice {
            dependency: dependency.clone(),
            project,
            version: resolved,
        });
    }
    Ok(choices)
}

pub type DependencyKey = (Source, String);

pub fn plan(
    version: &Version,
    mc: &str,
    loader: Loader,
    optional: &BTreeSet<DependencyKey>,
) -> Result<Vec<Version>> {
    cancel::check()?;
    let mut out = vec![version.clone()];
    let mut seen = HashSet::new();
    seen.insert((version.source, version.project_id.clone()));
    let mut queue = VecDeque::from([version.clone()]);
    let mut steps = 0usize;
    while let Some(current) = queue.pop_front() {
        cancel::check()?;
        steps += 1;
        if steps > 128 {
            return Err(Error::Install("前置依赖过多，已停止解析".into()));
        }
        for dependency in &current.dependencies {
            if matches!(
                dependency.kind,
                DependencyKind::Embedded | DependencyKind::Tool
            ) {
                continue;
            }
            if dependency.kind == DependencyKind::Incompatible {
                return Err(Error::Install(format!(
                    "{} 与当前 Mod 不兼容",
                    dependency
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("项目 {}", dependency.project_id))
                )));
            }
            let key = (dependency.source, dependency.project_id.clone());
            if dependency.kind == DependencyKind::Optional && !optional.contains(&key) {
                continue;
            }
            if !seen.insert(key) {
                continue;
            }
            let (project, resolved) = resolve_dependency(dependency, mc, loader)?
                .ok_or_else(|| Error::Install("前置依赖解析失败".into()))?;
            let Some(resolved) = resolved else {
                return Err(Error::Install(format!(
                    "找不到兼容的前置: {}",
                    project
                        .as_ref()
                        .map(|p| p.name.clone())
                        .or(dependency.name.clone())
                        .unwrap_or_else(|| dependency.project_id.clone())
                )));
            };
            out.push(resolved.clone());
            queue.push_back(resolved);
        }
    }
    Ok(out)
}

fn safe_mod_name(name: &str) -> Option<String> {
    let path = Path::new(name);
    if path.components().count() != 1
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    if name.is_empty() || name.contains('\0') || name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(name)
}

pub fn download_plan(
    mods_dir: &Path,
    versions: &[Version],
    overwrite: bool,
    sink: &Sink,
) -> Result<DownloadReport> {
    cancel::check()?;
    std::fs::create_dir_all(mods_dir).map_err(|e| crate::error::io(mods_dir, e))?;
    let mut specs = Vec::new();
    let mut skipped = Vec::new();
    for version in versions {
        cancel::check()?;
        let name = safe_mod_name(&version.file_name)
            .ok_or_else(|| Error::Install(format!("非法 Mod 文件名: {}", version.file_name)))?;
        let dest = mods_dir.join(&name);
        if dest.exists() && !overwrite {
            let size_ok = version
                .size
                .is_none_or(|size| dest.metadata().map(|m| m.len() == size).unwrap_or(false));
            if size_ok
                && version
                    .sha1
                    .as_deref()
                    .map(|sha1| crate::http::sha1_file(&dest).map(|v| v.eq_ignore_ascii_case(sha1)))
                    .transpose()?
                    .unwrap_or(true)
            {
                skipped.push(name);
                continue;
            }
            return Err(Error::Install(format!(
                "{} 已存在且内容不同，请先处理冲突",
                dest.display()
            )));
        }
        let url = version
            .download_url
            .clone()
            .ok_or_else(|| Error::Install(format!("{} 没有下载地址", version.file_name)))?;
        specs.push(FileSpec {
            url,
            dest,
            sha1: version.sha1.clone(),
            size: version.size,
            verify_existing: true,
        });
    }
    let count = specs.len();
    if count > 0 {
        (sink)(Progress::stage("下载 Mod 文件"));
        let http = Http::new("official")?;
        http.download_many_with(specs, "下载 Mod 文件", sink)?;
    }
    Ok(DownloadReport {
        downloaded: count,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modrinth_dependency_types() {
        let value = serde_json::json!({
            "project_id": "P",
            "version_id": "V",
            "file_name": "api.jar",
            "dependency_type": "required"
        });
        let dep = modrinth_dependency(&value).unwrap();
        assert_eq!(dep.kind, DependencyKind::Required);
        assert_eq!(dep.project_id, "P");
        assert_eq!(dep.version_id.as_deref(), Some("V"));
    }

    #[test]
    fn parses_curseforge_relations() {
        let required = serde_json::json!({"modId": 10, "relationType": 3});
        let optional = serde_json::json!({"modId": 11, "relationType": 2});
        let embedded = serde_json::json!({"modId": 12, "relationType": 1});
        let incompatible = serde_json::json!({"modId": 13, "relationType": 5});
        let item = serde_json::json!({
            "id": 1,
            "fileName": "test.jar",
            "downloadUrl": "https://example.com/test.jar",
            "fileLength": 10,
            "dependencies": [required, optional, embedded, incompatible]
        });
        let version = curseforge_version("42", ContentKind::Mod, &item).unwrap();
        assert_eq!(version.dependencies.len(), 3);
        assert!(
            version
                .dependencies
                .iter()
                .any(|d| d.kind == DependencyKind::Required)
        );
        assert!(
            version
                .dependencies
                .iter()
                .any(|d| d.kind == DependencyKind::Optional)
        );
        assert!(
            version
                .dependencies
                .iter()
                .any(|d| d.kind == DependencyKind::Incompatible)
        );
    }

    #[test]
    fn rejects_path_components_in_mod_names() {
        assert_eq!(safe_mod_name("example.jar").as_deref(), Some("example.jar"));
        assert!(safe_mod_name("../escape.jar").is_none());
        assert!(safe_mod_name("dir/example.jar").is_none());
    }
}
