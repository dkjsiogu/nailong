//! 真实网络冒烟测试：默认被 `#[ignore]` 跳过，手动执行：
//!
//! ```bash
//! cargo test -p nailong-core --test live_modrinth -- --ignored --nocapture
//! ```
//!
//! 它会真的打 Modrinth API、真的下载一个小 Mod，用来兜住“单元测试全绿但线上
//! 接口改字段”的情况。

use nailong_core::mods::{self, ContentKind, Loader, Source};
use nailong_core::progress;
use std::collections::BTreeSet;

#[test]
#[ignore = "需要网络，手动运行"]
fn search_plan_download_roundtrip() {
    let page = mods::search(
        "sodium",
        Some(Source::Modrinth),
        "1.21.1",
        Some(Loader::Fabric),
        ContentKind::Mod,
        0,
        5,
    )
    .expect("Modrinth 搜索失败");
    assert!(
        !page.hits.is_empty(),
        "搜索没有结果，warnings: {:?}",
        page.warnings
    );
    println!("search hits={} total={}", page.hits.len(), page.total);

    let project = page
        .hits
        .iter()
        .find(|p| p.slug == "sodium")
        .unwrap_or(&page.hits[0]);
    println!("project: {} ({})", project.name, project.slug);

    let versions = mods::versions(project, "1.21.1", Loader::Fabric).expect("版本列表失败");
    assert!(!versions.is_empty(), "没有 1.21.1 Fabric 版本");
    let latest = &versions[0];
    println!(
        "version: {} / {} / {:?} bytes",
        latest.version_number, latest.file_name, latest.size
    );

    let planned =
        mods::plan(latest, "1.21.1", Loader::Fabric, &BTreeSet::new()).expect("依赖解析失败");
    println!("plan: {} 个文件", planned.len());

    let root = std::env::temp_dir().join(format!("nailong-live-{}", std::process::id()));
    let mods_dir = root.join("mods");
    let report =
        mods::download_plan(&mods_dir, &planned, false, &progress::noop()).expect("下载失败");
    println!(
        "downloaded={} skipped={:?}",
        report.downloaded, report.skipped
    );

    for version in &planned {
        let path = mods_dir.join(&version.file_name);
        assert!(path.is_file(), "缺少文件: {}", path.display());
        if let Some(size) = version.size {
            let actual = std::fs::metadata(&path).expect("stat 失败").len();
            assert_eq!(actual, size, "大小不符: {}", path.display());
        }
    }

    // 第二次执行应该全部跳过，验证跳过逻辑不会重复下载。
    let again =
        mods::download_plan(&mods_dir, &planned, false, &progress::noop()).expect("二次下载失败");
    assert_eq!(again.downloaded, 0, "重复下载了已存在的文件");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "需要网络，手动运行"]
fn curseforge_search_smoke() {
    // CurseForge 依赖内置 key，缺 key 或限流时只打印，不让测试失败。
    match mods::search(
        "jei",
        Some(Source::CurseForge),
        "1.21.1",
        Some(Loader::NeoForge),
        ContentKind::Mod,
        0,
        3,
    ) {
        Ok(page) => println!(
            "curseforge hits={} total={} warnings={:?}",
            page.hits.len(),
            page.total,
            page.warnings
        ),
        Err(e) => println!("curseforge 搜索不可用: {e}"),
    }
}

#[test]
#[ignore = "需要网络，手动运行"]
fn browse_categories_return_results() {
    for (kind, category) in [
        (ContentKind::Mod, mods::categories(ContentKind::Mod)[0]),
        (
            ContentKind::ResourcePack,
            mods::categories(ContentKind::ResourcePack)[0],
        ),
        (
            ContentKind::Shader,
            mods::categories(ContentKind::Shader)[0],
        ),
    ] {
        let page = mods::browse(
            kind,
            Some(category),
            "1.21.1",
            Some(Loader::Fabric),
            mods::Sort::Popular,
            0,
            5,
        )
        .unwrap_or_else(|e| panic!("{kind:?} 浏览失败: {e}"));
        println!(
            "{:?} / {} -> {} hits, warnings={:?}",
            kind,
            category.label,
            page.hits.len(),
            page.warnings
        );
        assert!(
            !page.hits.is_empty(),
            "{kind:?} 的「{}」品类没有结果",
            category.label
        );
    }
}

#[test]
#[ignore = "需要网络，手动运行"]
fn one_click_install_writes_index() {
    let page = mods::search(
        "sodium",
        Some(Source::Modrinth),
        "1.21.1",
        Some(Loader::Fabric),
        ContentKind::Mod,
        0,
        5,
    )
    .expect("搜索失败");
    let project = page
        .hits
        .iter()
        .find(|p| p.slug == "sodium")
        .expect("没找到 sodium");

    let root = std::env::temp_dir().join(format!("nailong-1click-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let report = mods::install_project(
        &root,
        ContentKind::Mod,
        project,
        "1.21.1",
        Loader::Fabric,
        &progress::noop(),
    )
    .expect("一键安装失败");
    println!(
        "installed: downloaded={} skipped={:?}",
        report.downloaded, report.skipped
    );
    assert!(report.downloaded >= 1);

    let index = nailong_core::installed::load(&root);
    let entry = index
        .entries
        .iter()
        .find(|entry| entry.project_id.as_deref() == Some(project.id.as_str()))
        .expect("索引里没有记录安装的项目");
    assert!(root.join("mods").join(&entry.file).is_file());
    println!("index entry: {} ({:?})", entry.file, entry.version_number);

    let _ = std::fs::remove_dir_all(&root);
}
