use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::install::{fabric, forge, neoforge, vanilla};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
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

    let id = unique_instance_id(root, &index.name);
    let instance_dir = root.instance_dir(&id);
    let game_dir = instance_dir.join("minecraft");
    std::fs::create_dir_all(&game_dir).map_err(|e| crate::error::io(&game_dir, e))?;

    let result =
        import_files_and_overrides(root, archive, &mut zip, &index, &game_dir, mirror, sink);
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&instance_dir);
        return Err(e);
    }

    let meta = InstanceMeta {
        name: if index.name.trim().is_empty() {
            "整合包".to_string()
        } else {
            index.name.clone()
        },
        profile: profile.clone(),
        game_dir: "minecraft".to_string(),
        format: "mrpack".to_string(),
        source: archive
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    };
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
        id,
        name: meta.name,
        profile,
        game_dir,
    })
}

fn read_index(archive: &Path) -> Result<(Index, zip::ZipArchive<File>)> {
    let file = File::open(archive).map_err(|e| crate::error::io(archive, e))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| Error::Install(format!("打开整合包失败: {e}")))?;
    let mut text = String::new();
    zip.by_name("modrinth.index.json")
        .map_err(|_| Error::Install("不是有效的 Modrinth 整合包：缺少 modrinth.index.json".into()))?
        .read_to_string(&mut text)
        .map_err(|e| Error::Install(format!("读取整合包索引失败: {e}")))?;
    let index = serde_json::from_str(&text)
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

fn normalize_loader_version(mc: &str, version: &str) -> String {
    version
        .strip_prefix(&format!("{mc}-"))
        .unwrap_or(version)
        .to_string()
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
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| {
            let file = zip.by_index(i).ok()?;
            file.name().ok().map(|s| s.to_string())
        })
        .collect();
    for name in names {
        let Some(rest) = name
            .strip_prefix("overrides/")
            .or_else(|| name.strip_prefix("client-overrides/"))
        else {
            continue;
        };
        if rest.is_empty() || rest.ends_with('/') {
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
    }
    Ok(())
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
}
