use crate::blob;
use crate::cancel;
use crate::error::{Error, Result};
use crate::http::{FileSpec, Http};
use crate::manifest::{self, AssetIndex, Context, VersionJson, library_specs_for};
use crate::paths::{Root, write_atomic};
use crate::progress::{Progress, Sink, noop};
use serde::Deserialize;
use serde_json::Value;

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize)]
struct Manifest {
    versions: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    id: String,
    url: String,
}

pub fn install(root: &Root, mc: &str, mirror: &str) -> Result<()> {
    install_with(root, mc, mirror, &noop())
}

pub fn install_with(root: &Root, mc: &str, mirror: &str, sink: &Sink) -> Result<()> {
    cancel::check()?;
    install_runtime_with(root, mc, mirror, sink)?;
    install_assets_with(root, mc, mirror, sink)?;
    (sink)(Progress {
        stage: "安装完成".into(),
        files_done: 1,
        files_total: 1,
        bytes_done: 1,
        bytes_total: 1,
        indeterminate: false,
    });
    println!("✓ Minecraft {mc} 安装完成");
    Ok(())
}

/// Resolve the vanilla version JSON and write it to disk.  This is the only
/// step that needs the remote version manifest; everything else can be
/// downloaded from the URLs in this JSON.
pub fn prepare_version(root: &Root, mc: &str, mirror: &str, sink: &Sink) -> Result<VersionJson> {
    cancel::check()?;
    root.ensure()?;
    let http = Http::new(mirror)?;
    (sink)(Progress::stage("读取版本信息"));
    println!("→ 解析 Minecraft {mc} 版本信息…");
    let manifest: Manifest = serde_json::from_value(http.get_json(VERSION_MANIFEST)?)
        .map_err(|e| Error::Install(format!("版本清单格式异常: {e}")))?;
    let entry = manifest
        .versions
        .into_iter()
        .find(|v| v.id == mc)
        .ok_or_else(|| Error::VersionNotFound(mc.to_string()))?;

    let json_value = http.get_json(&entry.url)?;
    let vj: VersionJson = serde_json::from_value(json_value)
        .map_err(|e| Error::Install(format!("版本 JSON 解析失败: {e}")))?;
    write_atomic(
        &root.version_json(mc),
        serde_json::to_vec_pretty(&vj).unwrap().as_slice(),
    )?;
    Ok(vj)
}

pub fn install_runtime_with(root: &Root, mc: &str, mirror: &str, sink: &Sink) -> Result<()> {
    let vj = prepare_version(root, mc, mirror, sink)?;
    install_client_libs(root, mc, mirror, &vj, sink)
}

/// Client jar + libraries.  Loader processors need these, but assets do not.
pub fn install_client_libs(
    root: &Root,
    mc: &str,
    mirror: &str,
    vj: &VersionJson,
    sink: &Sink,
) -> Result<()> {
    cancel::check()?;
    root.ensure()?;
    let http = Http::new(mirror)?;
    let ctx = Context::current();

    let client = vj
        .downloads
        .client
        .as_ref()
        .ok_or_else(|| Error::Unsupported("版本没有 client 下载信息".into()))?;
    let client_spec = FileSpec {
        url: client.url.clone(),
        dest: root.client_jar(mc),
        sha1: client.sha1.clone(),
        size: client.size,
        verify_existing: true,
    };
    (sink)(Progress::stage("下载客户端"));
    println!("→ 下载客户端…");
    blob::download_specs(root, &http, vec![client_spec], "下载客户端", sink)?;

    let lib_specs = library_specs_for(root, &vj.libraries, &ctx);
    (sink)(Progress::stage("下载运行库"));
    println!("→ 下载依赖库（{} 个）…", lib_specs.len());
    blob::download_specs(root, &http, lib_specs, "下载运行库", sink)?;
    Ok(())
}

/// Asset index, asset objects and logging config.  Safe to run concurrently
/// with loader processor installation once the version JSON exists.
pub fn install_assets_with(root: &Root, mc: &str, mirror: &str, sink: &Sink) -> Result<()> {
    cancel::check()?;
    root.ensure()?;
    let vj = manifest::load(root, mc)?;
    let http = Http::new(mirror)?;

    let ai = vj
        .asset_index
        .as_ref()
        .ok_or_else(|| Error::Unsupported("版本缺少 assetIndex".into()))?;
    let index_path = root.indexes().join(format!("{}.json", ai.id));
    let index_url = ai
        .url
        .clone()
        .ok_or_else(|| Error::Unsupported("assetIndex 缺少 url".into()))?;
    (sink)(Progress::stage("读取资源索引"));
    println!("→ 下载资源索引 {}…", ai.id);
    http.download_with(
        FileSpec {
            url: index_url,
            dest: index_path.clone(),
            sha1: ai.sha1.clone(),
            size: ai.size,
            verify_existing: true,
        },
        "读取资源索引",
        sink,
    )?;

    let index: Value = serde_json::from_slice(
        &std::fs::read(&index_path).map_err(|e| crate::error::io(&index_path, e))?,
    )
    .map_err(|e| crate::error::json(&index_path, e))?;
    let objects = index
        .get("objects")
        .and_then(|v| v.as_object())
        .ok_or_else(|| Error::Install("资源索引缺少 objects".into()))?;
    let mut specs = Vec::with_capacity(objects.len());
    for (_name, obj) in objects {
        let Some(hash) = obj.get("hash").and_then(|v| v.as_str()) else {
            continue;
        };
        let size = obj.get("size").and_then(|v| v.as_u64());
        let dest = root.objects().join(&hash[0..2]).join(hash);
        specs.push(FileSpec {
            url: format!(
                "https://resources.download.minecraft.net/{}/{}",
                &hash[0..2],
                hash
            ),
            dest,
            sha1: Some(hash.to_string()),
            size,
            verify_existing: false,
        });
    }
    println!("→ 下载资源（{} 个文件）…", specs.len());
    http.download_many_with(specs, "下载游戏资源", sink)?;

    if let Some(logging) = vj.logging.as_ref().and_then(|l| l.client.as_ref()) {
        let dest = root.assets().join("log_configs").join(&logging.file.id);
        (sink)(Progress::stage("下载日志配置"));
        let _ = http.download_with(
            FileSpec {
                url: logging.file.url.clone(),
                dest,
                sha1: logging.file.sha1.clone(),
                size: logging.file.size,
                verify_existing: true,
            },
            "下载日志配置",
            sink,
        );
    }
    Ok(())
}

pub fn asset_index_for(root: &Root, ai: &AssetIndex) -> Result<Value> {
    let path = root.indexes().join(format!("{}.json", ai.id));
    let data = std::fs::read(&path).map_err(|e| crate::error::io(&path, e))?;
    serde_json::from_slice(&data).map_err(|e| crate::error::json(&path, e))
}
