use crate::builtin;
use crate::cancel;
use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::install::{fabric, forge, neoforge, vanilla};
use crate::manifest::{Context, VersionJson, library_specs_for};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub name: String,
    pub profile: String,
    pub game_dir: String,
    pub format: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct ImportedInstance {
    pub id: String,
    pub name: String,
    pub profile: String,
    pub game_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackFormat {
    Modrinth,
    CurseForge,
    Prism,
    Mcbbs,
    Hmcl,
    Atl,
}

enum CurseFileLookup {
    Ready(FileSpec),
    Blocked(String),
}

enum CurseApiLookup {
    File(ResolvedCurseFile),
    Blocked(String),
    Failed,
}

#[derive(Debug, Deserialize)]
struct Index {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    game: String,
    name: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    files: Vec<PackFile>,
}

#[derive(Debug, Deserialize)]
struct PackFile {
    path: String,
    #[serde(default)]
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(rename = "fileSize", default)]
    file_size: Option<u64>,
    #[serde(default)]
    env: Option<Env>,
}

#[derive(Debug, Deserialize)]
struct Env {
    #[serde(default)]
    client: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CurseManifest {
    #[serde(default)]
    name: Option<String>,
    minecraft: CurseMinecraft,
    #[serde(default)]
    files: Vec<CurseFile>,
    #[serde(default)]
    overrides: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CurseMinecraft {
    version: String,
    #[serde(rename = "modLoaders", default)]
    mod_loaders: Vec<CurseLoader>,
}

#[derive(Debug, Deserialize)]
struct CurseLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
struct CurseFile {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default)]
    required: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct McbbsManifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    addons: Vec<McbbsAddon>,
    #[serde(default)]
    files: Vec<McbbsFile>,
    #[serde(rename = "fileApi", default)]
    file_api: Option<String>,
    #[serde(default)]
    libraries: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct McbbsAddon {
    id: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum McbbsFile {
    Addon {
        path: String,
        #[serde(default)]
        hash: Option<String>,
    },
    Curse {
        #[serde(rename = "projectID")]
        project_id: u64,
        #[serde(rename = "fileID")]
        file_id: u64,
    },
}

#[derive(Debug, Deserialize)]
struct MmcPack {
    #[serde(default)]
    components: Vec<MmcComponent>,
}

#[derive(Debug, Deserialize)]
struct MmcComponent {
    uid: String,
    version: String,
}

pub fn import_pack(
    root: &Root,
    archive: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<ImportedInstance> {
    cancel::check()?;
    match detect_format(archive)? {
        PackFormat::Modrinth => import_mrpack(root, archive, mirror, sink),
        PackFormat::CurseForge => import_curseforge(root, archive, mirror, sink),
        PackFormat::Prism => import_prism(root, archive, mirror, sink),
        PackFormat::Mcbbs => import_mcbbs(root, archive, mirror, sink),
        PackFormat::Hmcl => import_hmcl(root, archive, mirror, sink),
        PackFormat::Atl => import_atl(root, archive, mirror, sink),
    }
}

fn detect_format(archive: &Path) -> Result<PackFormat> {
    let mut zip = open_zip(archive)?;
    let names = zip_names(&mut zip);
    if names.iter().any(|name| name == "modrinth.index.json") {
        return Ok(PackFormat::Modrinth);
    }
    if names.iter().any(|name| name == "mcbbs.packmeta") {
        return Ok(PackFormat::Mcbbs);
    }
    if names.iter().any(|name| name == "manifest.json") {
        let bytes = read_zip_entry(&mut zip, "manifest.json")?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Install(format!("manifest.json 解析失败: {e}")))?;
        if value.get("addons").is_some() {
            return Ok(PackFormat::Mcbbs);
        }
        return Ok(PackFormat::CurseForge);
    }
    if names.iter().any(|name| name == "mmc-pack.json") {
        return Ok(PackFormat::Prism);
    }
    if names.iter().any(|name| name == "modpack.json")
        && names.iter().any(|name| name == "minecraft/pack.json")
    {
        return Ok(PackFormat::Hmcl);
    }
    if names.iter().any(|name| name == "instance.json") {
        return Ok(PackFormat::Atl);
    }
    Err(Error::Unsupported(
        "无法识别的整合包格式；支持 Modrinth / CurseForge / Prism-MultiMC / HMCL / ATLauncher / MCBBS"
            .into(),
    ))
}

pub fn import_mrpack(
    root: &Root,
    archive: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<ImportedInstance> {
    root.ensure()?;
    let (index, mut zip) = read_index(archive)?;
    if index.format_version != 1 {
        return Err(Error::Unsupported(format!(
            "不支持的 mrpack 版本: {}",
            index.format_version
        )));
    }
    if index.game != "minecraft" {
        return Err(Error::Unsupported(format!(
            "不支持的整合包类型: {}",
            index.game
        )));
    }
    let mc = index
        .dependencies
        .get("minecraft")
        .cloned()
        .ok_or_else(|| Error::Install("整合包没有声明 Minecraft 版本".into()))?;

    (sink)(Progress::stage("安装整合包加载器"));
    let profile = install_loader(root, &mc, &index.dependencies, mirror, sink)?;
    let name = if index.name.trim().is_empty() {
        archive_stem(archive)
    } else {
        index.name.clone()
    };
    let (id, instance_dir, game_dir) = create_instance(root, &name, &profile, "mrpack", archive)?;

    let result =
        import_files_and_overrides(root, archive, &mut zip, &index, &game_dir, mirror, sink);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(e);
    }

    finish_instance(
        root,
        &id,
        &name,
        &profile,
        "mrpack",
        archive,
        game_dir,
        instance_dir,
        sink,
    )
}

fn import_curseforge(
    root: &Root,
    archive: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<ImportedInstance> {
    root.ensure()?;
    let mut zip = open_zip(archive)?;
    let manifest: CurseManifest =
        serde_json::from_slice(&read_zip_entry(&mut zip, "manifest.json")?)
            .map_err(|e| Error::Install(format!("CurseForge manifest.json 解析失败: {e}")))?;
    let mc = manifest.minecraft.version.trim().to_string();
    if mc.is_empty() {
        return Err(Error::Install(
            "CurseForge 整合包没有声明 Minecraft 版本".into(),
        ));
    }
    let loader = manifest
        .minecraft
        .mod_loaders
        .iter()
        .find(|loader| loader.primary)
        .or_else(|| manifest.minecraft.mod_loaders.first());
    (sink)(Progress::stage("安装整合包加载器"));
    let profile = match loader {
        Some(loader) => install_curse_loader(root, &mc, &loader.id, mirror, sink)?,
        None => {
            vanilla::install_with(root, &mc, mirror, sink)?;
            mc.clone()
        }
    };
    let name = manifest
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| archive_stem(archive));
    let (id, instance_dir, game_dir) =
        create_instance(root, &name, &profile, "curseforge", archive)?;

    let result = (|| -> Result<()> {
        import_curse_files(&manifest.files, &game_dir, mirror, sink)?;
        let overrides = manifest
            .overrides
            .clone()
            .unwrap_or_else(|| "overrides".to_string());
        let mut prefixes = vec![format!("{}/", overrides.trim_end_matches('/'))];
        if !prefixes.iter().any(|p| p == "client-overrides/") {
            prefixes.push("client-overrides/".to_string());
        }
        extract_prefixes(&mut zip, &game_dir, &prefixes, sink)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(e);
    }

    finish_instance(
        root,
        &id,
        &name,
        &profile,
        "curseforge",
        archive,
        game_dir,
        instance_dir,
        sink,
    )
}

fn import_prism(
    root: &Root,
    archive: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<ImportedInstance> {
    root.ensure()?;
    let mut zip = open_zip(archive)?;
    let pack: MmcPack = serde_json::from_slice(&read_zip_entry(&mut zip, "mmc-pack.json")?)
        .map_err(|e| Error::Install(format!("mmc-pack.json 解析失败: {e}")))?;
    let mc = pack
        .components
        .iter()
        .find(|component| component.uid == "net.minecraft")
        .map(|component| component.version.trim().to_string())
        .filter(|version| !version.is_empty())
        .ok_or_else(|| Error::Install("Prism/MultiMC 整合包没有声明 Minecraft 版本".into()))?;
    (sink)(Progress::stage("安装整合包加载器"));
    let profile = install_prism_loader(root, &mc, &pack.components, mirror, sink)?;
    let name = instance_cfg_name(&mut zip).unwrap_or_else(|| archive_stem(archive));
    let (id, instance_dir, game_dir) = create_instance(root, &name, &profile, "prism", archive)?;

    let names = zip_names(&mut zip);
    let prefix = if names.iter().any(|name| name.starts_with(".minecraft/")) {
        ".minecraft/"
    } else if names.iter().any(|name| name.starts_with("minecraft/")) {
        "minecraft/"
    } else {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(Error::Install(
            "Prism/MultiMC 整合包缺少 .minecraft/ 目录".into(),
        ));
    };
    let result = extract_prefixes(&mut zip, &game_dir, &[prefix.to_string()], sink);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(e);
    }

    finish_instance(
        root,
        &id,
        &name,
        &profile,
        "prism",
        archive,
        game_dir,
        instance_dir,
        sink,
    )
}

fn import_mcbbs(
    root: &Root,
    archive: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<ImportedInstance> {
    root.ensure()?;
    let mut zip = open_zip(archive)?;
    let manifest_name = if zip.by_name("mcbbs.packmeta").is_ok() {
        "mcbbs.packmeta"
    } else {
        "manifest.json"
    };
    let manifest: McbbsManifest = serde_json::from_slice(&read_zip_entry(&mut zip, manifest_name)?)
        .map_err(|e| Error::Install(format!("{manifest_name} 解析失败: {e}")))?;
    let mc = manifest
        .addons
        .iter()
        .find(|addon| addon.id == "net.minecraft")
        .map(|addon| addon.version.trim().to_string())
        .filter(|version| !version.is_empty())
        .ok_or_else(|| Error::Install("MCBBS 整合包没有声明 Minecraft 版本".into()))?;
    if !manifest.libraries.is_empty() {
        println!("! MCBBS 整合包含自定义 libraries，当前只安装 addon 加载器");
    }
    (sink)(Progress::stage("安装整合包加载器"));
    let profile = install_mcbbs_loader(root, &mc, &manifest.addons, mirror, sink)?;
    let name = manifest
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| archive_stem(archive));
    let (id, instance_dir, game_dir) = create_instance(root, &name, &profile, "mcbbs", archive)?;

    let result = (|| -> Result<()> {
        import_mcbbs_files(&manifest, &game_dir, mirror, sink)?;
        extract_prefixes(
            &mut zip,
            &game_dir,
            &["overrides/".to_string(), "client-overrides/".to_string()],
            sink,
        )
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(e);
    }

    finish_instance(
        root,
        &id,
        &name,
        &profile,
        "mcbbs",
        archive,
        game_dir,
        instance_dir,
        sink,
    )
}

fn install_mcbbs_loader(
    root: &Root,
    mc: &str,
    addons: &[McbbsAddon],
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    let find = |id: &str| {
        addons
            .iter()
            .find(|addon| addon.id == id)
            .map(|addon| addon.version.trim().to_string())
            .filter(|version| !version.is_empty())
    };
    if let Some(version) = find("net.fabricmc.fabric-loader") {
        return fabric::install_with(root, mc, &version, mirror, sink);
    }
    if let Some(version) = find("net.neoforged") {
        return neoforge::install_with(
            root,
            mc,
            &normalize_loader_version(mc, &version),
            mirror,
            sink,
        );
    }
    if let Some(version) = find("net.minecraftforge") {
        return forge::install_with(
            root,
            mc,
            &normalize_loader_version(mc, &version),
            mirror,
            sink,
        );
    }
    if find("org.quiltmc.quilt-loader").is_some() {
        return Err(Error::Unsupported("暂不支持 Quilt 整合包".into()));
    }
    vanilla::install_with(root, mc, mirror, sink)?;
    Ok(mc.to_string())
}

fn import_mcbbs_files(
    manifest: &McbbsManifest,
    game_dir: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<()> {
    if manifest.files.is_empty() {
        return Ok(());
    }
    let http = Http::new(mirror)?;
    let api_key = builtin::curseforge_api_key();
    (sink)(Progress::stage("解析 MCBBS 文件"));
    let mut specs = Vec::new();
    for file in &manifest.files {
        match file {
            McbbsFile::Curse {
                project_id,
                file_id,
            } => {
                let curse = CurseFile {
                    project_id: *project_id,
                    file_id: *file_id,
                    required: Some(true),
                };
                match curse_file_spec(&http, &curse, game_dir, api_key.as_deref()) {
                    CurseFileLookup::Ready(spec) => specs.push(spec),
                    CurseFileLookup::Blocked(label) => {
                        return Err(curse_blocked_error(&[label]));
                    }
                }
            }
            McbbsFile::Addon { path, hash } => {
                let Some(base) = manifest.file_api.as_deref() else {
                    println!("! MCBBS 文件缺少 fileApi，跳过: {path}");
                    continue;
                };
                let dest = match safe_join(game_dir, path) {
                    Ok(dest) => dest,
                    Err(e) => {
                        println!("! MCBBS 文件路径不安全，跳过: {e}");
                        continue;
                    }
                };
                specs.push(FileSpec {
                    url: format!(
                        "{}/{}",
                        base.trim_end_matches('/'),
                        path.trim_start_matches('/')
                    ),
                    dest,
                    sha1: hash.clone(),
                    size: None,
                    verify_existing: true,
                });
            }
        }
    }
    if !specs.is_empty() {
        (sink)(Progress::stage("下载整合包文件"));
        http.download_many_with(specs, "下载整合包文件", sink)?;
    }
    Ok(())
}

fn import_hmcl(root: &Root, archive: &Path, mirror: &str, sink: &Sink) -> Result<ImportedInstance> {
    let mut zip = open_zip(archive)?;
    let meta: Value = serde_json::from_slice(&read_zip_entry(&mut zip, "modpack.json")?)
        .map_err(|e| Error::Install(format!("HMCL modpack.json 解析失败: {e}")))?;
    let name = meta
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| archive_stem(archive));
    let game = meta
        .get("gameVersion")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let manifest: Value = serde_json::from_slice(&read_zip_entry(&mut zip, "minecraft/pack.json")?)
        .map_err(|e| Error::Install(format!("HMCL pack.json 解析失败: {e}")))?;
    let base = embedded_base(&manifest, game)
        .ok_or_else(|| Error::Install("HMCL 整合包没有声明 Minecraft 基础版本".into()))?;
    import_embedded(
        root,
        &mut zip,
        manifest,
        &name,
        &base,
        "minecraft/",
        "hmcl",
        archive,
        mirror,
        sink,
    )
}

fn import_atl(root: &Root, archive: &Path, mirror: &str, sink: &Sink) -> Result<ImportedInstance> {
    let mut zip = open_zip(archive)?;
    let manifest: Value = serde_json::from_slice(&read_zip_entry(&mut zip, "instance.json")?)
        .map_err(|e| Error::Install(format!("ATLauncher instance.json 解析失败: {e}")))?;
    let name = manifest
        .get("launcher")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            manifest
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| archive_stem(archive));
    let base = embedded_base(&manifest, None)
        .ok_or_else(|| Error::Install("ATLauncher 实例没有声明基础版本".into()))?;
    import_embedded(
        root, &mut zip, manifest, &name, &base, "", "atl", archive, mirror, sink,
    )
}

fn embedded_base(manifest: &Value, fallback: Option<String>) -> Option<String> {
    let from_inherit = manifest
        .get("inheritsFrom")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let from_jar = manifest.get("jar").and_then(|jar| {
        jar.as_str()
            .map(str::to_string)
            .or_else(|| jar.get("id").and_then(|v| v.as_str()).map(str::to_string))
    });
    from_inherit.or(from_jar).or(fallback)
}

#[allow(clippy::too_many_arguments)]
fn import_embedded(
    root: &Root,
    zip: &mut zip::ZipArchive<File>,
    mut manifest: Value,
    name: &str,
    base: &str,
    prefix: &str,
    format: &str,
    archive: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<ImportedInstance> {
    root.ensure()?;
    if manifest.get("mainClass").and_then(|v| v.as_str()).is_none() {
        return Err(Error::Unsupported(format!("{format} 整合包缺少 mainClass")));
    }
    (sink)(Progress::stage("安装 Minecraft 基础版本"));
    vanilla::install_with(root, base, mirror, sink)?;
    let profile = unique_loader_id(root, format, name);
    let obj = manifest
        .as_object_mut()
        .ok_or_else(|| Error::Install(format!("{format} manifest 不是对象")))?;
    obj.insert("id".to_string(), Value::String(profile.clone()));
    if obj.get("inheritsFrom").and_then(|v| v.as_str()).is_none() {
        obj.insert("inheritsFrom".to_string(), Value::String(base.to_string()));
    }
    let jar_id = obj.get("jar").and_then(|jar| {
        jar.as_str()
            .map(str::to_string)
            .or_else(|| jar.get("id").and_then(|v| v.as_str()).map(str::to_string))
    });
    obj.insert(
        "jar".to_string(),
        Value::String(jar_id.unwrap_or_else(|| base.to_string())),
    );
    let text = serde_json::to_vec_pretty(&manifest).unwrap();
    write_atomic(&root.version_json(&profile), &text)?;
    let version: VersionJson = serde_json::from_slice(&text)
        .map_err(|e| Error::Install(format!("{format} 版本解析失败: {e}")))?;

    (sink)(Progress::stage("下载整合包运行库"));
    let specs: Vec<FileSpec> = library_specs_for(root, &version.libraries, &Context::current())
        .into_iter()
        .filter(|spec| !spec.url.is_empty())
        .collect();
    let http = Http::new(mirror)?;
    http.download_many_with(specs, "下载整合包运行库", sink)?;

    let (id, instance_dir, game_dir) = create_instance(root, name, &profile, format, archive)?;
    let result = (|| -> Result<()> {
        extract_prefixes(zip, &game_dir, &[prefix.to_string()], sink)?;
        let _ = std::fs::remove_file(game_dir.join("pack.json"));
        let _ = std::fs::remove_file(game_dir.join("instance.json"));
        let _ = std::fs::remove_file(game_dir.join("instance.cfg"));
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(e);
    }

    finish_instance(
        root,
        &id,
        name,
        &profile,
        format,
        archive,
        game_dir,
        instance_dir,
        sink,
    )
}

fn unique_loader_id(root: &Root, prefix: &str, name: &str) -> String {
    let base = format!("{prefix}-{}", slug(name));
    let mut id = base.clone();
    let mut n = 2;
    while root.version_json(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn open_zip(archive: &Path) -> Result<zip::ZipArchive<File>> {
    let file = File::open(archive).map_err(|e| crate::error::io(archive, e))?;
    zip::ZipArchive::new(file).map_err(|e| Error::Install(format!("打开整合包失败: {e}")))
}

fn zip_names(zip: &mut zip::ZipArchive<File>) -> Vec<String> {
    (0..zip.len())
        .filter_map(|index| {
            let file = zip.by_index(index).ok()?;
            file.name().ok().map(|name| name.to_string())
        })
        .collect()
}

fn read_zip_entry<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut entry = zip
        .by_name(name)
        .map_err(|e| Error::Install(format!("整合包缺少 {name}: {e}")))?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Install(format!("读取 {name} 失败: {e}")))?;
    Ok(bytes)
}

fn read_index(archive: &Path) -> Result<(Index, zip::ZipArchive<File>)> {
    let mut zip = open_zip(archive)?;
    let bytes = read_zip_entry(&mut zip, "modrinth.index.json")?;
    let index = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Install(format!("整合包索引格式错误: {e}")))?;
    Ok((index, zip))
}

fn install_loader(
    root: &Root,
    mc: &str,
    deps: &BTreeMap<String, String>,
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    if let Some(loader) = deps.get("fabric-loader") {
        return fabric::install_with(root, mc, loader, mirror, sink);
    }
    if let Some(version) = deps.get("neoforge") {
        let version = normalize_loader_version(mc, version);
        return neoforge::install_with(root, mc, &version, mirror, sink);
    }
    if let Some(version) = deps.get("forge") {
        let version = normalize_loader_version(mc, version);
        return forge::install_with(root, mc, &version, mirror, sink);
    }
    if deps.contains_key("quilt-loader") {
        return Err(Error::Unsupported("暂不支持 Quilt 整合包".into()));
    }
    vanilla::install_with(root, mc, mirror, sink)?;
    Ok(mc.to_string())
}

fn install_curse_loader(
    root: &Root,
    mc: &str,
    id: &str,
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    let id = id.trim();
    if let Some(version) = id
        .strip_prefix("fabric-")
        .or_else(|| id.strip_prefix("fabric-loader-"))
    {
        return fabric::install_with(root, mc, version, mirror, sink);
    }
    if let Some(version) = id.strip_prefix("neoforge-") {
        let version = normalize_loader_version(mc, version);
        return neoforge::install_with(root, mc, &version, mirror, sink);
    }
    if let Some(version) = id.strip_prefix("forge-") {
        let version = normalize_loader_version(mc, version);
        return forge::install_with(root, mc, &version, mirror, sink);
    }
    if id.starts_with("quilt-") {
        return Err(Error::Unsupported("暂不支持 Quilt 整合包".into()));
    }
    // Some packs leave the loader id as just the loader version.
    if id.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        if mc.starts_with("1.20.1") {
            return forge::install_with(root, mc, &normalize_loader_version(mc, id), mirror, sink);
        }
        return neoforge::install_with(root, mc, &normalize_loader_version(mc, id), mirror, sink);
    }
    Err(Error::Unsupported(format!(
        "不支持的 CurseForge 加载器: {id}"
    )))
}

fn install_prism_loader(
    root: &Root,
    mc: &str,
    components: &[MmcComponent],
    mirror: &str,
    sink: &Sink,
) -> Result<String> {
    let find = |uid: &str| {
        components
            .iter()
            .find(|component| component.uid == uid)
            .map(|component| component.version.trim().to_string())
            .filter(|version| !version.is_empty())
    };
    if let Some(version) = find("net.fabricmc.fabric-loader") {
        return fabric::install_with(root, mc, &version, mirror, sink);
    }
    if let Some(version) = find("net.neoforged") {
        return neoforge::install_with(
            root,
            mc,
            &normalize_loader_version(mc, &version),
            mirror,
            sink,
        );
    }
    if let Some(version) = find("net.minecraftforge") {
        return forge::install_with(
            root,
            mc,
            &normalize_loader_version(mc, &version),
            mirror,
            sink,
        );
    }
    if find("org.quiltmc.quilt-loader").is_some() {
        return Err(Error::Unsupported("暂不支持 Quilt 整合包".into()));
    }
    vanilla::install_with(root, mc, mirror, sink)?;
    Ok(mc.to_string())
}

fn normalize_loader_version(mc: &str, version: &str) -> String {
    version
        .trim()
        .strip_prefix(&format!("{mc}-"))
        .unwrap_or(version)
        .to_string()
}

fn import_curse_files(
    files: &[CurseFile],
    game_dir: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    (sink)(Progress::stage("解析 CurseForge 文件"));
    let http = Http::new(mirror)?;
    let api_key = builtin::curseforge_api_key();
    let lookups: Vec<CurseFileLookup> = files
        .par_iter()
        .filter(|file| file.required != Some(false))
        .map(|file| curse_file_spec(&http, file, game_dir, api_key.as_deref()))
        .collect();
    let mut specs = Vec::new();
    let mut blocked = Vec::new();
    for lookup in lookups {
        match lookup {
            CurseFileLookup::Ready(spec) => specs.push(spec),
            CurseFileLookup::Blocked(label) => blocked.push(label),
        }
    }
    if !blocked.is_empty() {
        return Err(curse_blocked_error(&blocked));
    }
    if specs.is_empty() {
        return Ok(());
    }
    (sink)(Progress::stage("下载整合包文件"));
    http.download_many_with(specs, "下载整合包文件", sink)?;
    Ok(())
}

const CURSE_LOOKUP_INTERVAL: Duration = Duration::from_millis(120);
static CURSE_LOOKUP_GATE: Mutex<Option<Instant>> = Mutex::new(None);

/// CurseForge metadata is fetched from a shared worker pool. Space the starts
/// out so a large pack does not turn into a burst against the API or cfwidget.
fn throttle_curse_lookup() {
    let mut last = CURSE_LOOKUP_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(at) = *last {
        let elapsed = at.elapsed();
        if elapsed < CURSE_LOOKUP_INTERVAL {
            std::thread::sleep(CURSE_LOOKUP_INTERVAL - elapsed);
        }
    }
    *last = Some(Instant::now());
}

fn curse_blocked_error(blocked: &[String]) -> Error {
    let shown = blocked
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .join("；");
    let more = if blocked.len() > 6 {
        format!("；另有 {} 个", blocked.len() - 6)
    } else {
        String::new()
    };
    Error::Install(format!(
        "有 {} 个 CurseForge 文件被作者关闭了第三方分发，已停止导入：{}{}。请使用 CurseForge 官方启动器，或手动下载这些文件后重试。",
        blocked.len(),
        shown,
        more
    ))
}

fn curse_file_spec(
    http: &Http,
    file: &CurseFile,
    game_dir: &Path,
    api_key: Option<&str>,
) -> CurseFileLookup {
    throttle_curse_lookup();
    let resolved = match curse_api_file(http, file, api_key) {
        CurseApiLookup::File(resolved) => Some(resolved),
        CurseApiLookup::Blocked(name) => {
            return CurseFileLookup::Blocked(format!(
                "{name} (project {}, file {})",
                file.project_id, file.file_id
            ));
        }
        CurseApiLookup::Failed => None,
    };
    let (name, size, direct, sha1) = match resolved {
        Some(resolved) => resolved,
        None => match curse_widget_file(http, file) {
            Some((name, size)) => (name, size, None, None),
            None => (String::new(), None, None, None),
        },
    };
    let fallback = format!("mods/{}-{}.jar", file.project_id, file.file_id);
    let rel = if name.is_empty() {
        fallback
    } else {
        format!("mods/{name}")
    };
    let dest = safe_join(game_dir, &rel).unwrap_or_else(|_| {
        game_dir
            .join("mods")
            .join(format!("{}-{}.jar", file.project_id, file.file_id))
    });
    // Never synthesize a CDN path. Without an official download URL the
    // website endpoint is the only fallback, and it enforces the author's
    // distribution setting (including returning an error when disabled).
    let url = direct.unwrap_or_else(|| curse_download_page_url(file.project_id, file.file_id));
    CurseFileLookup::Ready(FileSpec {
        url,
        dest,
        sha1,
        size,
        verify_existing: true,
    })
}

type ResolvedCurseFile = (String, Option<u64>, Option<String>, Option<String>);

fn curse_api_file(http: &Http, file: &CurseFile, api_key: Option<&str>) -> CurseApiLookup {
    let Some(key) = api_key else {
        return CurseApiLookup::Failed;
    };
    let url = format!(
        "https://api.curseforge.com/v1/mods/{}/files/{}",
        file.project_id, file.file_id
    );
    let Ok(value) =
        http.get_json_with_headers(&url, &[("x-api-key", key), ("Accept", "application/json")])
    else {
        return CurseApiLookup::Failed;
    };
    curse_api_value(value)
}

fn curse_api_value(value: Value) -> CurseApiLookup {
    let Some(data) = value.get("data") else {
        return CurseApiLookup::Failed;
    };
    let Some(name) = data.get("fileName").and_then(|v| v.as_str()) else {
        return CurseApiLookup::Failed;
    };
    let Some(direct) = data
        .get("downloadUrl")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
    else {
        return CurseApiLookup::Blocked(name.to_string());
    };
    if !direct.starts_with("https://") {
        return CurseApiLookup::Failed;
    }
    let size = data.get("fileLength").and_then(|v| v.as_u64());
    let sha1 = data
        .get("hashes")
        .and_then(|v| v.as_array())
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("algo").and_then(|algo| algo.as_u64()) == Some(1))
                .and_then(|item| item.get("value").and_then(|v| v.as_str()))
                .map(str::to_string)
        });
    CurseApiLookup::File((name.to_string(), size, Some(direct.to_string()), sha1))
}

fn curse_download_page_url(project_id: u64, file_id: u64) -> String {
    format!("https://www.curseforge.com/api/v1/mods/{project_id}/files/{file_id}/download")
}

fn curse_widget_file(http: &Http, file: &CurseFile) -> Option<(String, Option<u64>)> {
    let url = format!(
        "https://cfwidget.com/{}?version={}",
        file.project_id, file.file_id
    );
    let html = http.get_text(&url).ok()?;
    let name = find_jar_name(&html)?;
    Some((name, None))
}

fn find_jar_name(html: &str) -> Option<String> {
    let end = html.find(".jar")? + 4;
    let start = html[..end]
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')))
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = &html[start..end];
    if name.len() > 4 && name.len() < 200 {
        Some(name.to_string())
    } else {
        None
    }
}

fn import_files_and_overrides(
    root: &Root,
    archive: &Path,
    zip: &mut zip::ZipArchive<File>,
    index: &Index,
    game_dir: &Path,
    mirror: &str,
    sink: &Sink,
) -> Result<()> {
    let _ = (root, archive);
    let mut specs = Vec::new();
    for file in &index.files {
        if file.env.as_ref().and_then(|e| e.client.as_deref()) == Some("unsupported") {
            continue;
        }
        let Some(url) = file.downloads.first().cloned() else {
            continue;
        };
        let dest = safe_join(game_dir, &file.path)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
        }
        specs.push(FileSpec {
            url,
            dest,
            sha1: file.hashes.get("sha1").cloned(),
            size: file.file_size,
            verify_existing: true,
        });
    }
    if !specs.is_empty() {
        let http = Http::new(mirror)?;
        http.download_many_with(specs, "下载整合包文件", sink)?;
    }

    (sink)(Progress::stage("解压整合包覆盖文件"));
    extract_prefixes(
        zip,
        game_dir,
        &["overrides/".to_string(), "client-overrides/".to_string()],
        sink,
    )
}

fn extract_prefixes(
    zip: &mut zip::ZipArchive<File>,
    game_dir: &Path,
    prefixes: &[String],
    sink: &Sink,
) -> Result<()> {
    (sink)(Progress::stage("解压整合包覆盖文件"));
    let names = zip_names(zip);
    let mut done = 0u64;
    for name in names {
        cancel::check()?;
        let Some(rest) = prefixes
            .iter()
            .find_map(|prefix| name.strip_prefix(prefix.as_str()))
        else {
            continue;
        };
        if rest.is_empty() || rest.ends_with('/') || name.starts_with("__MACOSX/") {
            continue;
        }
        let dest = safe_join(game_dir, rest)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
        }
        let mut entry = zip
            .by_name(&name)
            .map_err(|e| Error::Install(format!("读取覆盖文件 {name} 失败: {e}")))?;
        let mut out = Vec::new();
        entry
            .read_to_end(&mut out)
            .map_err(|e| Error::Install(format!("读取覆盖文件 {name} 失败: {e}")))?;
        std::fs::write(&dest, out).map_err(|e| crate::error::io(&dest, e))?;
        done += 1;
        if done.is_multiple_of(64) {
            (sink)(Progress {
                stage: "解压整合包覆盖文件".into(),
                files_done: done,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                indeterminate: true,
            });
        }
    }
    Ok(())
}

fn instance_cfg_name(zip: &mut zip::ZipArchive<File>) -> Option<String> {
    let mut bytes = Vec::new();
    zip.by_name("instance.cfg")
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("name=") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn create_instance(
    root: &Root,
    name: &str,
    profile: &str,
    format: &str,
    archive: &Path,
) -> Result<(String, PathBuf, PathBuf)> {
    let id = unique_instance_id(root, name);
    let instance_dir = root.instance_dir(&id);
    let game_dir = instance_dir.join("minecraft");
    std::fs::create_dir_all(&game_dir).map_err(|e| crate::error::io(&game_dir, e))?;
    let _ = (profile, format, archive);
    Ok((id, instance_dir, game_dir))
}

#[allow(clippy::too_many_arguments)]
fn finish_instance(
    root: &Root,
    id: &str,
    name: &str,
    profile: &str,
    format: &str,
    archive: &Path,
    game_dir: PathBuf,
    instance_dir: PathBuf,
    sink: &Sink,
) -> Result<ImportedInstance> {
    let meta = InstanceMeta {
        name: name.to_string(),
        profile: profile.to_string(),
        game_dir: "minecraft".to_string(),
        format: format.to_string(),
        source: archive
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    };
    let _ = root;
    write_atomic(
        &instance_dir.join("instance.json"),
        &serde_json::to_vec_pretty(&meta).unwrap(),
    )?;
    (sink)(Progress {
        stage: "安装完成".into(),
        files_done: 1,
        files_total: 1,
        bytes_done: 1,
        bytes_total: 1,
        indeterminate: false,
    });
    println!(
        "✓ 整合包 {} 导入完成（{}）",
        meta.name,
        instance_dir.display()
    );
    Ok(ImportedInstance {
        id: id.to_string(),
        name: meta.name,
        profile: profile.to_string(),
        game_dir,
    })
}

fn archive_stem(archive: &Path) -> String {
    archive
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "整合包".to_string())
}

fn unique_instance_id(root: &Root, name: &str) -> String {
    let base = slug(name);
    let mut id = base.clone();
    let mut n = 2;
    while root.instance_dir(&id).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().take(40) {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "modpack".to_string()
    } else {
        out
    }
}

fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.replace('\\', "/");
    if rel.starts_with('/') {
        return Err(Error::Install(format!("整合包路径不安全: {rel}")));
    }
    if rel.is_empty() {
        return Err(Error::Install("整合包包含空路径".into()));
    }
    let path = Path::new(&rel);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::Install(format!("整合包路径不安全: {rel}")));
    }
    Ok(base.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_pack_paths() {
        let base = Path::new("/tmp/instance/minecraft");
        assert!(safe_join(base, "mods/a.jar").is_ok());
        assert!(safe_join(base, "../escape.jar").is_err());
        assert!(safe_join(base, "/absolute.jar").is_err());
        assert!(safe_join(base, "mods\\a.jar").is_ok());
    }

    #[test]
    fn slug_handles_unicode_and_spaces() {
        assert_eq!(slug("Fabulously Optimized"), "fabulously-optimized");
        assert_eq!(slug("整合包"), "modpack");
        assert_eq!(slug("My_Pack-2"), "my_pack-2");
    }

    #[test]
    fn parses_curse_manifest() {
        let manifest: CurseManifest = serde_json::from_str(
            r#"{
                "name":"Test Pack",
                "minecraft":{"version":"1.21.1","modLoaders":[{"id":"neoforge-21.1.209","primary":true}]},
                "files":[{"projectID":238222,"fileID":9268986,"required":true}],
                "overrides":"overrides"
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.minecraft.version, "1.21.1");
        assert_eq!(manifest.files[0].project_id, 238222);
        assert_eq!(manifest.files[0].required, Some(true));
        assert!(manifest.minecraft.mod_loaders[0].primary);
    }

    #[test]
    fn curse_fallback_uses_site_download_endpoint() {
        assert_eq!(
            curse_download_page_url(238222, 9268986),
            "https://www.curseforge.com/api/v1/mods/238222/files/9268986/download"
        );
    }

    #[test]
    fn curse_api_without_download_url_is_blocked() {
        let value = serde_json::json!({
            "data": {"fileName": "restricted.jar", "downloadUrl": null}
        });
        assert!(matches!(
            curse_api_value(value),
            CurseApiLookup::Blocked(name) if name == "restricted.jar"
        ));
    }

    #[test]
    fn curse_api_with_download_url_is_ready() {
        let value = serde_json::json!({
            "data": {
                "fileName": "allowed.jar",
                "fileLength": 123,
                "downloadUrl": "https://edge.forgecdn.net/files/1/2/allowed.jar?api-key=x",
                "hashes": [{"algo": 1, "value": "abc"}]
            }
        });
        match curse_api_value(value) {
            CurseApiLookup::File((name, size, url, sha1)) => {
                assert_eq!(name, "allowed.jar");
                assert_eq!(size, Some(123));
                assert!(url.as_deref().is_some_and(|u| u.starts_with("https://")));
                assert_eq!(sha1.as_deref(), Some("abc"));
            }
            _ => panic!("expected a ready file"),
        }
    }

    #[test]
    fn blocked_curse_files_produce_actionable_error() {
        let message =
            curse_blocked_error(&["example.jar (project 1, file 2)".to_string()]).to_string();
        assert!(message.contains("example.jar"));
        assert!(message.contains("第三方分发"));
        assert!(message.contains("CurseForge 官方启动器"));
    }

    #[test]
    fn parses_prism_components() {
        let pack: MmcPack = serde_json::from_str(
            r#"{"components":[
                {"uid":"net.minecraft","version":"1.21.1"},
                {"uid":"net.neoforged","version":"21.1.209"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(pack.components.len(), 2);
        assert_eq!(pack.components[1].uid, "net.neoforged");
    }
}
