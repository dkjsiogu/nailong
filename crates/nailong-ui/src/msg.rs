//! 后台线程回到界面的唯一入口：所有结果都是一条 `Msg`，在 `poll` 里按种类交给对应的状态。

use crate::prelude::*;

impl App {
    pub(crate) fn poll(&mut self) {
        let mut got_cf = Vec::new();
        let mut cf_done = false;
        if let Some(rx) = &self.content.cf_rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => got_cf.push(item),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        cf_done = true;
                        break;
                    }
                }
            }
        }
        if cf_done {
            self.content.cf_rx = None;
        }
        for (kind, result) in got_cf {
            self.content.cf_loading.remove(&kind);
            match result {
                Ok(items) => {
                    self.content.cf_categories.insert(kind, items);
                }
                Err(_) => {
                    self.content.cf_categories.insert(kind, Vec::new());
                }
            }
        }
        let mut messages = Vec::new();
        if let Some(rx) = &self.worlds.rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.login.rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.rollback.rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.install.catalog_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.content.rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        if let Some(rx) = &self.content.scan_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }
        for msg in messages {
            match msg {
                Msg::Progress(p) => {
                    if self.busy.is_some_and(Task::shows_progress)
                        && let Some(activity) = &mut self.activity
                    {
                        if activity.progress.stage != p.stage {
                            self.log.push(p.stage.clone());
                        }
                        activity.update(p);
                    }
                }
                Msg::Catalog(result) => {
                    self.install.catalog_loading = false;
                    self.install.catalog_rx = None;
                    match result {
                        Ok(cat) => self.install.catalog = Some(cat),
                        Err(e) => {
                            self.install.catalog = Some(catalog::Catalog::fallback());
                            self.install.catalog_error =
                                Some("在线版本不可用，使用内置版本".into());
                            self.log.push(format!("版本列表获取失败: {e}"));
                        }
                    }
                    if self.busy.is_none() {
                        self.pick_defaults();
                    }
                }
                Msg::InstallDone(id) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.activity = None;
                    self.rx = None;
                    self.show_install = false;
                    self.error = None;
                    self.install.repair_target = None;
                    self.notice = Some("安装完成".into());
                    self.refresh();
                    if self.rows.iter().any(|r| r.id == id) {
                        self.selected = Some(id);
                    }
                }
                Msg::InstallFailed(e) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.activity = None;
                    self.rx = None;
                    self.show_install = true;
                    self.log.push(format!("安装未完成: {e}"));
                    if e.contains("操作已取消") {
                        self.error = None;
                        self.notice = Some("安装已取消，已下载的部分会保留。".into());
                    } else {
                        self.error = Some(e);
                    }
                    self.refresh();
                }
                Msg::InstallCancelled => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.activity = None;
                    self.rx = None;
                    self.show_install = true;
                    self.error = None;
                    self.notice =
                        Some("安装已取消，已下载的部分会保留；重新开始会检查并补全。".into());
                    self.log.push("安装已取消".into());
                    self.refresh();
                }
                Msg::ModsSearch {
                    query,
                    offset,
                    result,
                } => {
                    self.content.loading = false;
                    self.content.rx = None;
                    if query != self.content.query.trim() {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            if offset == 0 {
                                self.content.results = page.hits;
                            } else {
                                for project in page.hits {
                                    if !self.content.results.iter().any(|existing| {
                                        existing.source == project.source
                                            && existing.id == project.id
                                    }) {
                                        self.content.results.push(project);
                                    }
                                }
                            }
                            self.content.offset = self.content.results.len();
                            self.content.total = page.total.max(self.content.offset as u64);
                            self.content.warnings = page.warnings;
                            self.content.error = None;
                            self.content.searching = true;
                            let projects = self.content.results.clone();
                            self.request_icons(projects);
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::ModsVersions { project, result } => {
                    self.content.rx = None;
                    let Some(selected) = &self.content.selected else {
                        continue;
                    };
                    if project != format!("{:?}:{}", selected.source, selected.id) {
                        continue;
                    }
                    self.content.versions_loading = false;
                    match result {
                        Ok(mut versions) => {
                            self.content.versions = std::mem::take(&mut versions);
                            self.content.error = None;
                            if let Some(first) = self.content.versions.first().cloned() {
                                self.select_mods_version(first);
                            }
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::ModsDeps { version, result } => {
                    self.content.deps_loading = false;
                    self.content.rx = None;
                    let Some(selected) = &self.content.version else {
                        continue;
                    };
                    if version != format!("{:?}:{}", selected.source, selected.id) {
                        continue;
                    }
                    match result {
                        Ok(choices) => {
                            self.content.choices = choices;
                            self.content.error = None;
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::ModsDone(note) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.activity = None;
                    self.rx = None;
                    self.content.quick = None;
                    self.content.notice = Some(note);
                    self.content.error = None;
                    self.refresh();
                    if self.show_mods {
                        self.start_mods_scan();
                    }
                }
                Msg::ModsFailed(e) => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.activity = None;
                    self.rx = None;
                    self.content.quick = None;
                    self.content.error = Some(content_error(e));
                }
                Msg::ModsCancelled => {
                    self.busy = None;
                    self.cancel_requested = false;
                    cancel::reset();
                    self.activity = None;
                    self.rx = None;
                    self.content.quick = None;
                    self.content.notice = Some("Mod 下载已取消".into());
                    self.content.error = None;
                }
                Msg::ModsInstalled(inst) => {
                    self.content.installed_loading = false;
                    self.content.scan_rx = None;
                    match inst {
                        Ok(entries) => {
                            self.content.installed_keys = entries
                                .iter()
                                .filter_map(|entry| {
                                    Some(format!(
                                        "{:?}:{}",
                                        entry.source?,
                                        entry.project_id.as_ref()?
                                    ))
                                })
                                .collect();
                            self.content.installed = entries;
                            self.content.updates.clear();
                            self.content.error = None;
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::ModsUpdates(updates) => {
                    self.content.updates_loading = false;
                    self.content.scan_rx = None;
                    match updates {
                        Ok(updates) => {
                            self.content.updates = updates;
                            self.content.error = None;
                            self.content.notice =
                                Some(format!("检查完成，{} 个可更新", self.content.updates.len()));
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::BrowseMore {
                    sort,
                    offset,
                    result,
                } => {
                    self.content.loading = false;
                    self.content.rx = None;
                    if self.content.shelf != Some(sort) {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            if offset == 0 {
                                self.content.results = page.hits;
                            } else {
                                for project in page.hits {
                                    if !self.content.results.iter().any(|existing| {
                                        existing.source == project.source
                                            && existing.id == project.id
                                    }) {
                                        self.content.results.push(project);
                                    }
                                }
                            }
                            self.content.offset = self.content.results.len();
                            self.content.total = page.total.max(self.content.offset as u64);
                            self.content.warnings = page.warnings;
                            self.content.error = None;
                            let projects = self.content.results.clone();
                            self.request_icons(projects);
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::Browse {
                    kind,
                    category,
                    source,
                    result,
                } => {
                    if kind != self.content.kind
                        || category != self.content.category
                        || source != self.content.source_filter
                    {
                        continue;
                    }
                    self.content.browse_loading = false;
                    self.content.rx = None;
                    match result {
                        Ok(shelves) => {
                            self.request_icons(
                                shelves.iter().flat_map(|shelf| shelf.projects.clone()),
                            );
                            self.content.browse = shelves;
                            self.content.error = None;
                        }
                        Err(e) => self.content.error = Some(content_error(e)),
                    }
                }
                Msg::SavesDone(result) => {
                    self.worlds.busy = false;
                    self.home.cache = None;
                    self.home.verified = None;
                    self.play.tuning_cache.clear();
                    self.worlds.rx = None;
                    if self.worlds.auto_backup_running {
                        self.worlds.auto_backup_running = false;
                        self.worlds.backup.last_run = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or_default();
                        let _ = snapshots::save_config(&self.root, &self.worlds.backup);
                    }
                    match result {
                        Ok(note) => {
                            self.worlds.notice = Some(note);
                            self.worlds.error = None;
                            self.reload_saves();
                        }
                        Err(e) => self.worlds.error = Some(e),
                    }
                }
                Msg::RollbackDone(result) => {
                    self.rollback.busy = false;
                    self.rollback.rx = None;
                    match result {
                        Ok(note) => {
                            self.rollback.notice = Some(note);
                            self.rollback.error = None;
                            self.reload_rollback_points();
                            self.refresh_installed_keys();
                            self.start_mods_scan();
                        }
                        Err(e) => self.rollback.error = Some(e),
                    }
                }
                Msg::LoginBrowser(url) => {
                    self.login.url = Some(url);
                }
                Msg::LoginDone(result) => {
                    self.login.busy = false;
                    self.login.rx = None;
                    match result {
                        Ok(account) => {
                            let name = account.name.clone();
                            self.accounts
                                .add(nailong_core::accounts::Account::from_microsoft(account));
                            if let Err(e) = self.accounts.save(&self.root) {
                                self.error = Some(e.to_string());
                            }
                            self.login.url = None;
                            self.login.error = None;
                            self.notice = Some(format!("已登录 {name}"));
                        }
                        Err(e) => {
                            self.login.url = None;
                            self.log.push(format!("登录失败: {e}"));
                            self.error = None;
                            self.login.error = Some(friendly_login_error(&e));
                        }
                    }
                }
                Msg::LaunchDone(code, reason) => {
                    if let Some(last) = &mut self.play.last {
                        last.outcome = log::Outcome::Exited(code);
                    }
                    // 启动前可能刷新过凭证，重新读一遍；这局截的图也该出现在截图页了。
                    self.accounts = nailong_core::accounts::Accounts::load(&self.root);
                    self.gallery.stale = true;
                    self.busy = None;
                    self.play.running_row = None;
                    // 每玩完一局就给世界留一份快照，比按时间间隔更贴近“坏了能救”。
                    if self.worlds.backup.enabled && !self.worlds.busy {
                        self.start_auto_backup();
                    }
                    self.rx = None;
                    self.play.tuning_cache.clear();
                    if code == 0 {
                        self.notice = Some("游戏已退出".into());
                    } else {
                        self.log.push(format!("游戏异常退出（代码 {code}）"));
                        self.error = Some(match reason {
                            Some(reason) => format!("游戏异常退出（代码 {code}）：{reason}"),
                            None => {
                                format!("游戏异常退出（代码 {code}），日志里标红的是出错的地方")
                            }
                        });
                    }
                }
                // 账户的后台结果走自己的通道，在 poll_accounts 里处理。
                Msg::ServerChecked(..)
                | Msg::ExternalLogin(_)
                | Msg::AccountAdded(_)
                | Msg::Face(..) => {}
                Msg::NeedRelogin(id, reason) => {
                    if let Some(last) = &mut self.play.last {
                        last.outcome = log::Outcome::Failed(reason.clone());
                    }
                    self.busy = None;
                    self.play.running_row = None;
                    self.rx = None;
                    self.accounts = nailong_core::accounts::Accounts::load(&self.root);
                    self.log.push(format!("账户要重新登录：{reason}"));
                    self.relogin(&id, Some(reason.clone()));
                }
                Msg::LaunchFailed(e) => {
                    self.accounts = nailong_core::accounts::Accounts::load(&self.root);
                    if let Some(last) = &mut self.play.last {
                        last.outcome = log::Outcome::Failed(e.clone());
                    }
                    self.busy = None;
                    self.play.running_row = None;
                    self.rx = None;
                    self.error = Some(format!("启动失败：{e}"));
                    self.log.push(format!("启动失败: {e}"));
                }
                Msg::BenchDone(result) => {
                    self.finish_bench();
                    match result {
                        Ok(store) => {
                            let summary = store
                                .javas
                                .iter()
                                .map(|java| {
                                    let tiers = java
                                        .tiers
                                        .iter()
                                        .map(|tier| {
                                            format!("{} {}", tier.label, tier.collector.label())
                                        })
                                        .collect::<Vec<_>>()
                                        .join("、");
                                    format!("Java {} {tiers}", java.major)
                                })
                                .collect::<Vec<_>>()
                                .join("；");
                            self.log.push(format!("测试完成：{summary}"));
                            for java in &store.javas {
                                for tier in &java.tiers {
                                    self.log.extend(tier.notes.iter().cloned());
                                }
                            }
                            self.notice =
                                Some(format!("测试完成：{summary}。之后「自动」回收器按它选"));
                            self.env.bench = Some(store);
                            self.play.tuning_cache.clear();
                            self.show_settings = true;
                            self.show_tuning = true;
                        }
                        Err(e) => {
                            self.log.push(format!("测试失败: {e}"));
                            self.error = Some(format!("测试没完成：{e}"));
                        }
                    }
                }
                Msg::JavaDone(result) => self.finish_java_download(result),
                Msg::JavaCancelled => {
                    self.finish_java_download(Err(String::new()));
                }
                Msg::BenchCancelled => {
                    self.finish_bench();
                    self.notice = Some("测试已停止，之前的结论没有改动".into());
                }
            }
        }
    }
}

pub(crate) enum Msg {
    Progress(Progress),
    Catalog(Result<catalog::Catalog, String>),
    InstallDone(String),
    InstallFailed(String),
    InstallCancelled,
    ModsSearch {
        query: String,
        offset: usize,
        result: Result<mods::SearchPage, String>,
    },
    ModsVersions {
        project: String,
        result: Result<Vec<mods::Version>, String>,
    },
    ModsDeps {
        version: String,
        result: Result<Vec<mods::DependencyChoice>, String>,
    },
    ModsDone(String),
    ModsFailed(String),
    ModsCancelled,
    ModsInstalled(Result<Vec<installed::InstalledEntry>, String>),
    ModsUpdates(Result<Vec<installed::UpdateCandidate>, String>),
    Browse {
        kind: mods::ContentKind,
        category: Option<mods::CategoryFilter>,
        source: Option<mods::Source>,
        result: Result<Vec<Shelf>, String>,
    },
    BrowseMore {
        sort: mods::Sort,
        offset: usize,
        result: Result<mods::SearchPage, String>,
    },
    SavesDone(Result<String, String>),
    RollbackDone(Result<String, String>),
    LoginBrowser(String),
    LoginDone(Result<auth::Account, String>),
    /// 退出码，以及从游戏输出里认出的原因。
    LaunchDone(i32, Option<String>),
    LaunchFailed(String),
    /// 开始游戏前发现账户凭证失效：账户编号和原因。
    NeedRelogin(String, String),
    /// 外置登录：检查认证服务器（玩家的输入，结果）。
    ServerChecked(String, Result<nailong_core::yggdrasil::Server, String>),
    /// 外置登录：密码对了，但有好几个角色要选。
    ExternalLogin(Result<Box<Pending>, String>),
    /// 账户加好了（外置登录或选完角色）。
    AccountAdded(Result<Box<nailong_core::accounts::Account>, String>),
    /// 皮肤地址和切出来的脸。
    Face(String, Option<egui::ColorImage>),
    BenchDone(Result<bench::Store, String>),
    BenchCancelled,
    /// Java 下载完：成功时是「Java 21（Microsoft 21.0.7）」这样的说明。
    JavaDone(Result<String, String>),
    JavaCancelled,
}
