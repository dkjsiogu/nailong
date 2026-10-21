use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::install::{fabric, forge, neoforge, vanilla};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

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
    match detect_format(archive)? {
        PackFormat::Modrinth => import_mrpack(root, archive, mirror, sink),
        PackFormat::CurseForge => import_curseforge(root, archive, mirror, sink),
        PackFormat::Prism => import_prism(root, archive, mirror, sink),
    }
}

fn detect_format(archive: &Path) -> Result<PackFormat> {
    let mut zip = open_zip(archive)?;
    let names = zip_names(&mut zip);
    if names.iter().any(|name| name == "modrinth.index.json") {
        return Ok(PackFormat::Modrinth);
    }
    if names.iter().any(|name| name == "manifest.json") {
        return Ok(PackFormat::CurseForge);
    }
    if names.iter().any(|name| name == "mmc-pack.json") {
        return Ok(PackFormat::Prism);
    }
    Err(Error::Unsupported(
        "无法识别的整合包格式；支持 Modrinth .mrpack、CurseForge .zip、Prism/MultiMC 导出 .zip"
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
    (sink)(Progress::stage("解析 CurseForge 文件"));
    let specs: Vec<FileSpec> = files
        .par_iter()
        .filter(|file| file.required != Some(false))
        .map(|file| curse_file_spec(&http, file, game_dir))
        .collect();
    (sink)(Progress::stage("下载整合包文件"));
    http.download_many_with(specs, "下载整合包文件", sink)?;
    Ok(())
}

fn curse_file_spec(http: &Http, file: &CurseFile, game_dir: &Path) -> FileSpec {
    let fallback = format!("mods/{}-{}.jar", file.project_id, file.file_id);
    let meta_url = format!(
        "https://www.curseforge.com/api/v1/mods/{}/files/{}",
        file.project_id, file.file_id
    );
    let widget = curse_widget_file(http, file);
    let (name, size, direct) = if widget.0.is_empty() {
        match http.get_json(&meta_url) {
            Ok(value) => {
                let data = value.get("data").unwrap_or(&value);
                let name = data
                    .get("fileName")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.is_empty())
                    .unwrap_or("")
                    .to_string();
                let size = data.get("fileLength").and_then(|v| v.as_u64());
                let direct = data
                    .get("downloadUrl")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.is_empty())
                    .map(str::to_string);
                (name, size, direct)
            }
            Err(_) => (String::new(), None, None),
        }
    } else {
        widget
    };
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
    let url = direct.unwrap_or_else(|| {
        if name.is_empty() {
            format!(
                "https://www.curseforge.com/api/v1/mods/{}/files/{}/download",
                file.project_id, file.file_id
            )
        } else {
            let first = file.file_id / 1000;
            let last = file.file_id % 1000;
            format!("https://mediafilez.forgecdn.net/files/{first}/{last:03}/{name}")
        }
    });
    FileSpec {
        url,
        dest,
        sha1: None,
        size,
        verify_existing: false,
    }
}

fn curse_widget_file(http: &Http, file: &CurseFile) -> (String, Option<u64>, Option<String>) {
    let url = format!(
        "https://cfwidget.com/{}?version={}",
        file.project_id, file.file_id
    );
    let Ok(html) = http.get_text(&url) else {
        return (String::new(), None, None);
    };
    let Some(name) = find_jar_name(&html) else {
        return (String::new(), None, None);
    };
    let first = file.file_id / 1000;
    let last = file.file_id % 1000;
    let direct = format!("https://mediafilez.forgecdn.net/files/{first}/{last:03}/{name}");
    (name, None, Some(direct))
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
