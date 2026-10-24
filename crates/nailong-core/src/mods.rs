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

/// 浏览时的排序方式。搜索仍然用 `Relevance`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Popular,
    Newest,
    Relevance,
}

impl Sort {
    fn modrinth_index(self) -> &'static str {
        match self {
            Self::Popular => "downloads",
            Self::Newest => "newest",
            Self::Relevance => "relevance",
        }
    }

    /// CurseForge sortField：6 = 总下载量，11 = 发布日期，2 = 热度。
    fn curseforge_field(self) -> Option<u32> {
        match self {
            Self::Popular => Some(6),
            Self::Newest => Some(11),
            Self::Relevance => None,
        }
    }
}

/// 一个品类胶囊。`label` 是界面文字，`slug` 是 Modrinth 的分类标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Category {
    pub label: &'static str,
    pub slug: &'static str,
}

const MOD_CATEGORIES: &[Category] = &[
    Category {
        label: "优化",
        slug: "optimization",
    },
    Category {
        label: "科技",
        slug: "technology",
    },
    Category {
        label: "冒险",
        slug: "adventure",
    },
    Category {
        label: "魔法",
        slug: "magic",
    },
    Category {
        label: "装饰",
        slug: "decoration",
    },
    Category {
        label: "生物",
        slug: "mobs",
    },
    Category {
        label: "世界",
        slug: "worldgen",
    },
    Category {
        label: "工具",
        slug: "utility",
    },
    Category {
        label: "存储",
        slug: "storage",
    },
    Category {
        label: "库",
        slug: "library",
    },
];

const RESOURCE_PACK_CATEGORIES: &[Category] = &[
    Category {
        label: "写实",
        slug: "realistic",
    },
    Category {
        label: "简约",
        slug: "simplistic",
    },
    Category {
        label: "原版风",
        slug: "vanilla-like",
    },
    Category {
        label: "主题",
        slug: "themed",
    },
    Category {
        label: "音频",
        slug: "audio",
    },
    Category {
        label: "界面",
        slug: "gui",
    },
    Category {
        label: "方块",
        slug: "blocks",
    },
    Category {
        label: "生物",
        slug: "entities",
    },
];

const SHADER_CATEGORIES: &[Category] = &[
    Category {
        label: "写实",
        slug: "realistic",
    },
    Category {
        label: "奇幻",
        slug: "fantasy",
    },
    Category {
        label: "卡通",
        slug: "cartoon",
    },
    Category {
        label: "原版风",
        slug: "vanilla-like",
    },
    Category {
        label: "低配",
        slug: "potato",
    },
    Category {
        label: "光追",
        slug: "path-tracing",
    },
    Category {
        label: "反射",
        slug: "reflections",
    },
    Category {
        label: "阴影",
        slug: "shadows",
    },
];

pub fn categories(kind: ContentKind) -> &'static [Category] {
    match kind {
        ContentKind::Mod => MOD_CATEGORIES,
        ContentKind::ResourcePack => RESOURCE_PACK_CATEGORIES,
        ContentKind::Shader => SHADER_CATEGORIES,
    }
}

/// 当前构建能不能用 CurseForge（没有内置 key 时直接跳过，不弹警告）。
pub fn curseforge_available() -> bool {
    builtin::curseforge_api_key().is_some()
}

/// 一次浏览的品类过滤：Modrinth 用 slug，CurseForge 用官方 categoryId。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryFilter {
    pub modrinth: Option<&'static str>,
    pub curseforge: Option<u32>,
}

/// CurseForge 官方品类树的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurseCategory {
    pub id: u32,
    pub name: String,
    pub slug: String,
    pub display_index: i64,
    /// 子品类会带上父品类 id；界面默认只显示顶层品类，避免一次铺三四十个胶囊。
    pub parent: Option<u32>,
}

/// 拉 CurseForge 的真实品类树，界面直接用它当胶囊。
pub fn curseforge_categories(kind: ContentKind) -> Result<Vec<CurseCategory>> {
    cancel::check()?;
    let key = curse_key()?;
    let url = query_url(
        &format!("{CURSEFORGE_API}/categories"),
        &[
            ("gameId", CURSEFORGE_GAME_ID.to_string()),
            ("classId", kind.curseforge_class().to_string()),
        ],
    )?;
    let value = Http::new("official")?.get_json_with_headers(&url, &curse_headers(&key))?;
    let mut out: Vec<CurseCategory> = value
        .get("data")
        .and_then(|v| v.as_array())
        .map(|items| items.iter().filter_map(curse_category).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| {
        a.display_index
            .cmp(&b.display_index)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

fn curse_category(value: &Value) -> Option<CurseCategory> {
    Some(CurseCategory {
        id: value.get("id")?.as_u64()?.try_into().ok()?,
        name: value.get("name")?.as_str()?.to_string(),
        slug: value
            .get("slug")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        display_index: value
            .get("displayIndex")
            .and_then(|v| v.as_i64())
            .unwrap_or_default(),
        parent: value
            .get("parentCategoryId")
            .and_then(|v| v.as_u64())
            .and_then(|id| id.try_into().ok()),
    })
}

fn normalize_category(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 把 CurseForge 的品类名/slug 归一成我们的中文标签和 Modrinth slug。
///
/// 没收录的品类返回 None，界面就原样显示 CurseForge 的名字。
pub fn category_alias(slug: &str, name: &str) -> Option<(&'static str, Option<&'static str>)> {
    for key in [normalize_category(slug), normalize_category(name)] {
        let hit = match key.as_str() {
            "technology" | "tech" => ("科技", Some("technology")),
            "magic" => ("魔法", Some("magic")),
            "adventureandrp" | "adventureandrpg" | "adventure" => ("冒险", Some("adventure")),
            "decoration" | "decorations" => ("装饰", Some("decoration")),
            "worldgen" | "worldgeneration" => ("世界", Some("worldgen")),
            "mobs" | "mob" => ("生物", Some("mobs")),
            "storage" => ("存储", Some("storage")),
            "library" | "libraryandapis" | "libraries" => ("库", Some("library")),
            "optimization" | "performance" => ("优化", Some("optimization")),
            "utility" | "serverutility" | "miscellaneous" => ("工具", Some("utility")),
            "food" => ("食物", Some("food")),
            "equipment" | "armor" | "armortoolsandweapons" => ("装备", Some("equipment")),
            "energy" => ("能源", None),
            "redstone" => ("红石", None),
            "transport" | "playertransport" | "transportation" => ("交通", Some("transportation")),
            "addons" => ("附属", None),
            "cosmetic" => ("外观", None),
            "mapandinformation" => ("地图信息", None),
            "oresandresources" => ("矿物", None),
            "structures" => ("结构", None),
            "biomes" => ("生物群系", Some("worldgen")),
            "dimensions" => ("维度", Some("worldgen")),
            "minigame" | "minigames" => ("小游戏", Some("minigame")),
            "economy" => ("经济", Some("economy")),
            "gamemechanics" => ("机制", Some("game-mechanics")),
            "social" => ("社交", Some("social")),
            "management" => ("管理", Some("management")),
            "cursed" => ("整活", Some("cursed")),
            _ => continue,
        };
        return Some(hit);
    }
    None
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

/// 一次查询的固定条件，避免内部函数参数爆炸。
struct Query<'a> {
    text: &'a str,
    modrinth_category: Option<&'a str>,
    curseforge_category: Option<u32>,
    sort: Sort,
    mc: Option<&'a str>,
    loader: Option<Loader>,
    kind: ContentKind,
}

pub fn search(
    query: &str,
    source: Option<Source>,
    mc: Option<&str>,
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
        match search_modrinth(
            Query {
                text: query,
                modrinth_category: None,
                curseforge_category: None,
                sort: Sort::Relevance,
                mc,
                loader,
                kind,
            },
            offset,
            limit,
        ) {
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
    if want(Source::CurseForge) && curseforge_available() {
        match search_curseforge(
            Query {
                text: query,
                modrinth_category: None,
                curseforge_category: None,
                sort: Sort::Relevance,
                mc,
                loader,
                kind,
            },
            offset,
            limit,
        ) {
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

/// 一次浏览请求：品类、来源、版本、加载器和分页。
pub struct BrowseOptions<'a> {
    pub kind: ContentKind,
    pub category: Option<CategoryFilter>,
    pub source: Option<Source>,
    pub mc: Option<&'a str>,
    pub loader: Option<Loader>,
    pub sort: Sort,
    pub offset: usize,
    pub limit: usize,
}

/// 浏览：按品类/热度拿内容，不需要输入关键词。
///
/// 品类和来源都可过滤：Modrinth 用分类 slug，CurseForge 用官方 `categoryId`。
pub fn browse(options: BrowseOptions<'_>) -> Result<SearchPage> {
    cancel::check()?;
    let BrowseOptions {
        kind,
        category,
        source,
        mc,
        loader,
        sort,
        offset,
        limit,
    } = options;
    let limit = limit.clamp(1, 50);
    let mut page = SearchPage {
        hits: Vec::new(),
        total: 0,
        warnings: Vec::new(),
    };
    let mut last_error = None;
    let want = |candidate: Source| source.is_none_or(|wanted| wanted == candidate);

    if want(Source::Modrinth) {
        match search_modrinth(
            Query {
                text: "",
                modrinth_category: category.and_then(|c| c.modrinth),
                curseforge_category: None,
                sort,
                mc,
                loader,
                kind,
            },
            offset,
            limit,
        ) {
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
    if want(Source::CurseForge) && curseforge_available() {
        match search_curseforge(
            Query {
                text: "",
                modrinth_category: None,
                curseforge_category: category.and_then(|c| c.curseforge),
                sort,
                mc,
                loader,
                kind,
            },
            offset,
            limit,
        ) {
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
    if sort == Sort::Relevance {
        page.hits
            .sort_by_key(|project| std::cmp::Reverse(project.downloads));
    }
    Ok(page)
}

fn search_modrinth(query: Query<'_>, offset: usize, limit: usize) -> Result<SearchPage> {
    let mut facets = vec![vec![format!("project_type:{}", query.kind.modrinth_type())]];
    if let Some(mc) = query.mc {
        facets.push(vec![format!("versions:{mc}")]);
    }
    if query.kind == ContentKind::Mod
        && let Some(loader) = query.loader
    {
        facets.push(vec![format!("categories:{}", loader.modrinth_category())]);
    }
    if let Some(category) = query.modrinth_category {
        facets.push(vec![format!("categories:{category}")]);
    }
    let facets = serde_json::to_string(&facets)
        .map_err(|e| Error::Install(format!("Modrinth facets 序列化失败: {e}")))?;
    let url = query_url(
        &format!("{MODRINTH_API}/search"),
        &[
            ("query", query.text.to_string()),
            ("facets", facets),
            ("index", query.sort.modrinth_index().to_string()),
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
                .filter_map(|value| modrinth_project(value, query.kind))
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

fn search_curseforge(query: Query<'_>, offset: usize, limit: usize) -> Result<SearchPage> {
    let key = curse_key()?;
    let mut params = vec![
        ("gameId", CURSEFORGE_GAME_ID.to_string()),
        ("classId", query.kind.curseforge_class().to_string()),
        ("index", offset.to_string()),
        ("pageSize", limit.min(50).to_string()),
        ("sortOrder", "desc".to_string()),
    ];
    if let Some(field) = query.sort.curseforge_field() {
        params.push(("sortField", field.to_string()));
    }
    if let Some(category) = query.curseforge_category {
        params.push(("categoryId", category.to_string()));
    }
    if let Some(mc) = query.mc {
        params.push(("gameVersion", mc.to_string()));
    }
    if query.kind == ContentKind::Mod
        && let Some(loader) = query.loader
    {
        params.push(("modLoaderType", loader.curseforge_id().to_string()));
    }
    if !query.text.trim().is_empty() {
        params.push(("searchFilter", query.text.trim().to_string()));
    }
    let url = query_url(&format!("{CURSEFORGE_API}/mods/search"), &params)?;
    let value = Http::new("official")?.get_json_with_headers(&url, &curse_headers(&key))?;
    let hits = value
        .get("data")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|value| curseforge_project(value, query.kind))
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

/// 拉项目图标（PNG/JPEG 原始字节）。
pub fn fetch_icon(url: &str) -> Result<Vec<u8>> {
    Http::new("official")?.get_bytes(url)
}

/// 一键安装：挑最新兼容版本、带上必需前置、下载并写进内容索引。
///
/// 界面上的卡片按钮直接调这个，不需要先选版本。
pub fn install_project(
    game_dir: &Path,
    kind: ContentKind,
    project: &Project,
    mc: &str,
    loader: Loader,
    sink: &Sink,
) -> Result<DownloadReport> {
    cancel::check()?;
    let versions = versions(project, mc, loader)?;
    let latest = versions.first().ok_or_else(|| {
        Error::Install(format!(
            "{} 没有适配 {mc} / {} 的版本",
            project.name,
            loader.label()
        ))
    })?;
    let plan = plan(latest, mc, loader, &BTreeSet::new())?;
    let dir = game_dir.join(kind.folder());
    let report = download_plan(&dir, &plan, false, sink)?;
    crate::installed::record_versions(game_dir, kind, &plan)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_tables_are_unique_and_nonempty() {
        for kind in [
            ContentKind::Mod,
            ContentKind::ResourcePack,
            ContentKind::Shader,
        ] {
            let items = categories(kind);
            assert!(!items.is_empty(), "{kind:?} 没有品类");
            let mut labels = HashSet::new();
            let mut slugs = HashSet::new();
            for category in items {
                assert!(!category.label.is_empty());
                assert!(!category.slug.is_empty());
                assert!(
                    labels.insert(category.label),
                    "重复品类名: {}",
                    category.label
                );
                assert!(slugs.insert(category.slug), "重复 slug: {}", category.slug);
            }
        }
    }

    #[test]
    fn parses_curseforge_categories() {
        let value = serde_json::json!({
            "id": 447,
            "name": "Technology",
            "slug": "technology",
            "displayIndex": 3,
            "parentCategoryId": null
        });
        let category = curse_category(&value).unwrap();
        assert_eq!(category.id, 447);
        assert_eq!(category.name, "Technology");
        assert_eq!(category.slug, "technology");
        assert_eq!(category.display_index, 3);
        assert!(category.parent.is_none());
        let child = serde_json::json!({
            "id": 448,
            "name": "Tech Sub",
            "slug": "tech-sub",
            "parentCategoryId": 447
        });
        assert_eq!(curse_category(&child).unwrap().parent, Some(447));
    }

    #[test]
    fn aliases_curseforge_categories() {
        let (label, modrinth) = category_alias("technology", "Technology").unwrap();
        assert_eq!(label, "科技");
        assert_eq!(modrinth, Some("technology"));
        let (label, modrinth) = category_alias("world-gen", "World Gen").unwrap();
        assert_eq!(label, "世界");
        assert_eq!(modrinth, Some("worldgen"));
        let (label, _) = category_alias("adventure-and-rpg", "Adventure and RPG").unwrap();
        assert_eq!(label, "冒险");
        assert!(category_alias("未知品类", "Unknown Thing").is_none());
    }

    #[test]
    fn sort_maps_to_api_fields() {
        assert_eq!(Sort::Popular.modrinth_index(), "downloads");
        assert_eq!(Sort::Newest.modrinth_index(), "newest");
        assert_eq!(Sort::Popular.curseforge_field(), Some(6));
        assert_eq!(Sort::Newest.curseforge_field(), Some(11));
        assert_eq!(Sort::Relevance.curseforge_field(), None);
    }

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
