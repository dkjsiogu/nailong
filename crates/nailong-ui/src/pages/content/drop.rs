//! 拖放导入：按内容识别拖进来的文件，整批一个恢复点。

use crate::prelude::*;

impl App {
    /// 拖进来的内容装到哪：在内容页就是上面选的目标，其他页面是当前版本。
    pub(crate) fn drop_target(&self) -> Option<(Row, PathBuf)> {
        if self.show_mods {
            return self.content.row.clone().zip(self.content.game_dir.clone());
        }
        let row = self.selected_row()?;
        let dir = row
            .game_dir
            .clone()
            .unwrap_or_else(|| self.root.version_dir(&row.profile));
        Some((row, dir))
    }

    /// 把拖进来的文件（或文件夹里的文件）分成内容、整合包和认不出的。
    pub(crate) fn plan_drop(&self, paths: &[PathBuf]) -> DropPlan {
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                let mut inside: Vec<PathBuf> = std::fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|p| {
                        p.is_file() && installed::classify(p) != installed::Dropped::Unknown
                    })
                    .collect();
                inside.sort();
                files.extend(inside);
            } else {
                files.push(path.clone());
            }
        }
        // 在资源包、光影页拖进不认识的 zip，就按当前页的类别装。
        let page_kind = (self.show_mods && self.content.kind != mods::ContentKind::Mod)
            .then_some(self.content.kind);
        let mut plan = DropPlan {
            target: self.drop_target(),
            ..Default::default()
        };
        for file in files {
            match installed::classify(&file) {
                installed::Dropped::Content(kind) => plan.items.push((file, kind)),
                installed::Dropped::Modpack => plan.packs.push(file),
                installed::Dropped::Archive => match page_kind {
                    Some(kind) => plan.items.push((file, kind)),
                    None => plan.unknown.push(file),
                },
                installed::Dropped::Unknown => plan.unknown.push(file),
            }
        }
        plan
    }

    /// 悬停时的一句话：会装进哪、装几个；不能装时说为什么。
    pub(crate) fn describe_drop(&self, plan: &DropPlan) -> (String, bool) {
        if plan.items.is_empty() {
            return match (plan.packs.len(), plan.unknown.len()) {
                (1, 0) => ("松开导入整合包，自动装好加载器".into(), self.busy.is_none()),
                (0, _) => (
                    "认不出这些文件：支持 Mod .jar、资源包和光影 .zip、整合包 .mrpack".into(),
                    false,
                ),
                _ => ("整合包一次拖一个".into(), false),
            };
        }
        let Some((row, _)) = &plan.target else {
            return ("先选一个版本：内容只会装进当前版本".into(), false);
        };
        if let Some(reason) = self.write_block(Some(&row.id)) {
            return (reason.into(), false);
        }
        if plan
            .items
            .iter()
            .any(|(_, kind)| *kind == mods::ContentKind::Mod)
            && mod_loader(row.loader).is_none()
        {
            return (
                format!("「{}」是原版，不能装 Mod", short_text(&row.label, 20)),
                false,
            );
        }
        let mut extra = String::new();
        if !plan.packs.is_empty() || !plan.unknown.is_empty() {
            extra = format!(
                "（另有 {} 个不是内容，会跳过）",
                plan.packs.len() + plan.unknown.len()
            );
        }
        (
            format!(
                "松开导入 {} → {}{extra}",
                count_kinds(plan.items.iter().map(|(_, kind)| *kind)),
                short_text(&row.label, 24)
            ),
            true,
        )
    }

    /// 导入一批拖进来或选中的文件：整批只留一个恢复点，已经装过的跳过。
    pub(crate) fn import_dropped(&mut self, plan: DropPlan) {
        let on_content = self.show_mods;
        let mut imported: Vec<mods::ContentKind> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        let mut marked = false;
        if !plan.items.is_empty() {
            let (reason, ok) = self.describe_drop(&DropPlan {
                target: plan.target.clone(),
                items: plan.items.clone(),
                ..Default::default()
            });
            match (&plan.target, ok) {
                (Some((row, game_dir)), true) => {
                    let fresh: Vec<&(PathBuf, mods::ContentKind)> = plan
                        .items
                        .iter()
                        .filter(|(path, kind)| {
                            let file = path
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let known = installed::contains(game_dir, *kind, &file);
                            if known {
                                skipped.push(file);
                            }
                            !known
                        })
                        .collect();
                    if !fresh.is_empty() {
                        if row.instance.is_none()
                            && !self.root.isolated_marker(&row.profile).exists()
                        {
                            let _ = std::fs::create_dir_all(game_dir);
                            match std::fs::write(self.root.isolated_marker(&row.profile), b"1") {
                                Ok(()) => marked = true,
                                Err(e) => problems.push(format!("启用版本隔离失败: {e}")),
                            }
                        }
                        let loader = loader_state_files(&self.root, row, game_dir);
                        if problems.is_empty() {
                            if let Err(e) = rollback::backup_extra(game_dir, "导入内容前", &loader)
                            {
                                problems.push(format!("无法创建恢复点: {e}"));
                            } else {
                                for (path, kind) in fresh {
                                    match installed::import_local(game_dir, *kind, path) {
                                        Ok(entry) => {
                                            self.content.recent.insert(entry.file.clone());
                                            names.push(entry.name);
                                            imported.push(*kind);
                                        }
                                        Err(e) => problems.push(e.to_string()),
                                    }
                                }
                            }
                        }
                    }
                }
                _ => problems.push(reason),
            }
        }
        let lone_pack = plan.items.is_empty() && plan.unknown.is_empty() && plan.packs.len() == 1;
        if !plan.unknown.is_empty() {
            let shown = plan
                .unknown
                .iter()
                .take(2)
                .filter_map(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
                .collect::<Vec<_>>()
                .join("、");
            problems.push(format!(
                "认不出 {shown}{}：支持 Mod .jar、资源包和光影 .zip、整合包 .mrpack",
                if plan.unknown.len() > 2 { " 等" } else { "" }
            ));
        }
        if !plan.packs.is_empty() && !lone_pack {
            problems.push("整合包要单独拖进来，一次一个".into());
        }

        let mut notice = None;
        if !imported.is_empty() {
            let mut text = format!(
                "已导入 {}：{}",
                count_kinds(imported.iter().copied()),
                short_text(&names.join("、"), 60)
            );
            if !skipped.is_empty() {
                text.push_str(&format!("；{} 个已经装过，跳过", skipped.len()));
            }
            notice = Some(text);
        } else if !skipped.is_empty() {
            notice = Some(format!("{} 个文件已经装过，没有重复导入", skipped.len()));
        }
        let error = (!problems.is_empty()).then(|| problems.join("；"));
        if on_content {
            self.content.notice = notice;
            self.content.error = error;
        } else {
            self.notice = notice;
            self.error = error;
        }
        if lone_pack {
            if self.busy.is_none() {
                self.start_modpack(plan.packs[0].clone());
            } else {
                self.error = Some(self.busy_reason().into());
            }
            return;
        }
        if imported.is_empty() {
            return;
        }
        if marked {
            self.refresh();
        }
        if on_content {
            // 新导入的要看得见：清掉筛选；全是另一类时切到那一类。
            self.content.status = StatusFilter::All;
            self.content.filter.clear();
            if let Some(&kind) = imported.first()
                && imported.iter().all(|k| *k == kind)
                && kind != self.content.kind
            {
                self.content.kind = kind;
            }
            self.start_mods_scan();
        } else {
            self.home.cache = None;
            self.home.verified = None;
            self.play.tuning_cache.clear();
        }
    }

    /// 文件管理器里改过内容目录、回到窗口时自动重扫。
    pub(crate) fn rescan_if_changed(&mut self) {
        if !(self.show_mods && self.content.show_installed) || self.content.scan_rx.is_some() {
            return;
        }
        let Some(game_dir) = &self.content.game_dir else {
            return;
        };
        if content_signature(game_dir) != self.content.dir_sig {
            self.content.notice = None;
            self.start_mods_scan();
        }
    }

    /// 拖着文件经过窗口时，盖一层说明会装到哪。安装页有自己的拖放区，不盖。
    pub(crate) fn drop_overlay(&mut self, ui: &mut egui::Ui) {
        let hovered: Vec<PathBuf> = ui.ctx().input(|i| {
            i.raw
                .hovered_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        let dragging = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        if !dragging || self.show_install {
            self.content.drop_preview = None;
            return;
        }
        let stale = self
            .content
            .drop_preview
            .as_ref()
            .is_none_or(|(paths, _, _)| *paths != hovered);
        if stale {
            let (text, ok) = if hovered.is_empty() {
                ("松开导入".to_string(), true)
            } else {
                self.describe_drop(&self.plan_drop(&hovered))
            };
            self.content.drop_preview = Some((hovered, text, ok));
        }
        let Some((_, text, ok)) = &self.content.drop_preview else {
            return;
        };
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, t::tint(t::bg(), 0.88));
        let zone = rect.shrink(24.0);
        let color = if *ok { t::accent() } else { t::warn() };
        painter.rect_stroke(
            zone,
            motion::R,
            egui::Stroke::new(1.5, t::tint(color, 0.7)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            zone.center() - egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            if *ok {
                "松开导入"
            } else {
                "这里不能导入"
            },
            egui::FontId::new(22.0, display_family()),
            t::ink(),
        );
        painter.text(
            zone.center() + egui::vec2(0.0, 18.0),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(14.0),
            color,
        );
    }
}

/// 例如「3 个 Mod」「2 个 Mod、1 个资源包」。
pub(crate) fn count_kinds(kinds: impl Iterator<Item = mods::ContentKind>) -> String {
    let mut counts: Vec<(mods::ContentKind, usize)> = Vec::new();
    for kind in kinds {
        match counts.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((kind, 1)),
        }
    }
    counts
        .iter()
        .map(|(kind, n)| {
            let label = kind.label();
            if label.is_ascii() {
                format!("{n} 个 {label}")
            } else {
                format!("{n} 个{label}")
            }
        })
        .collect::<Vec<_>>()
        .join("、")
}

/// 三个内容目录的文件名、大小和修改时间合成一个数，用来发现外面的改动。
pub(crate) fn content_signature(game_dir: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for kind in [
        mods::ContentKind::Mod,
        mods::ContentKind::ResourcePack,
        mods::ContentKind::Shader,
    ] {
        let mut items: Vec<(String, u64, u64)> = std::fs::read_dir(game_dir.join(kind.folder()))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                Some((
                    entry.file_name().to_string_lossy().to_string(),
                    meta.len(),
                    modified,
                ))
            })
            .collect();
        items.sort();
        items.hash(&mut hasher);
    }
    hasher.finish()
}
