//! 应用的骨架：全部状态、启动时的初始化，以及每一帧的流程（收消息 → 快捷键 → 侧栏 → 页面 → 弹窗）。

use crate::prelude::*;

impl App {
    pub(crate) fn new(root: Root) -> Self {
        let javas = java::find_all(&root);
        let java_ok = javas.iter().any(|j| j.major >= 21);
        let java_label = javas.iter().find(|j| j.major >= 21).map(|j| j.label());
        let account = auth::load(&root);
        let backup_cfg = snapshots::load_config(&root);
        let prefs = load_ui_prefs(&root);
        let (icon_jobs, icon_job_rx) = mpsc::channel::<(String, String)>();
        let (icon_done, icon_rx) = mpsc::channel::<(String, Vec<u8>)>();
        std::thread::spawn(move || {
            while let Ok((key, url)) = icon_job_rx.recv() {
                if let Ok(bytes) = mods::fetch_icon(&url)
                    && icon_done.send((key, bytes)).is_err()
                {
                    break;
                }
            }
        });
        let mut app = Self {
            play: PlayState {
                tuning_cache: HashMap::new(),
                versions: BTreeMap::new(),
                last: None,
                started: None,
                running_row: None,
                launch_after_java: None,
            },
            env: Env {
                javas,
                extra_javas: Vec::new(),
                java_ok,
                java_label,
                machine: tuning::Machine::detect(),
                bench: None,
            },
            home: HomeState {
                cache: None,
                scan: None,
                verified: None,
                shots: None,
            },
            prefs,
            install: InstallForm {
                catalog: None,
                catalog_loading: false,
                catalog_error: None,
                catalog_rx: None,
                mc: "1.21.1".into(),
                kind: LoaderKind::Fabric,
                fabric: "0.19.5".into(),
                neoforge: "21.1.209".into(),
                forge: "52.1.20".into(),
                repair_target: None,
            },
            rollback: RollbackState {
                open: false,
                points: Vec::new(),
                busy: false,
                confirm: None,
                notice: None,
                error: None,
                rx: None,
            },
            login: LoginState {
                open: false,
                url: None,
                rx: None,
                busy: false,
                error: None,
            },
            worlds: WorldsState {
                row: None,
                selected: None,
                list: Vec::new(),
                confirm: None,
                confirm_backup: None,
                snapshots: HashMap::new(),
                rx: None,
                busy: false,
                notice: None,
                error: None,
                backup: backup_cfg,
                auto_backup_running: false,
            },
            content: ContentState {
                row: None,
                game_dir: None,
                query: String::new(),
                filter: String::new(),
                status: StatusFilter::All,
                recent: HashSet::new(),
                installed_dir: None,
                dir_sig: 0,
                delete_confirm: None,
                kind: mods::ContentKind::Mod,
                browse: Vec::new(),
                browse_loading: false,
                category: None,
                source_filter: None,
                mc: None,
                loader: None,
                chips_expanded: false,
                shelf: None,
                searching: false,
                quick: None,
                installed_keys: HashSet::new(),
                show_installed: false,
                installed_loading: false,
                installed: Vec::new(),
                updates_loading: false,
                updates: Vec::new(),
                results: Vec::new(),
                total: 0,
                offset: 0,
                warnings: Vec::new(),
                loading: false,
                selected: None,
                versions: Vec::new(),
                versions_loading: false,
                version: None,
                choices: Vec::new(),
                optional: BTreeSet::new(),
                deps_loading: false,
                notice: None,
                error: None,
                rx: None,
                scan_rx: None,
                icons: HashMap::new(),
                icons_pending: HashSet::new(),
                cf_categories: HashMap::new(),
                cf_loading: HashSet::new(),
                cf_rx: None,
                icon_jobs,
                icon_rx,
                focus_search: false,
                drop_preview: None,
            },
            root,
            rows: Vec::new(),
            selected: None,
            show_tuning: false,
            bench_confirm: false,
            launch_edit: None,
            show_install: false,
            show_manage: false,
            show_mods: false,
            show_base: false,
            delete_confirm: None,
            window_focused: true,
            busy: None,
            cancel_requested: false,
            activity: None,
            update: UpdateState::default(),
            error: None,
            notice: None,
            log: vec!["启动器已就绪".into()],
            log_view: None,
            show_saves: false,
            account,
            show_settings: false,
            pending_install: None,
            version_cache: None,
            page_key: 255,
            page_enter_at: 0.0,
            modal_key: 0,
            modal_started: 0.0,
            rx: None,
        };
        app.play.versions = launchcfg::load_all(&app.root);
        app.refresh();
        app.env.bench = bench::load(&app.root);
        {
            // 0.35 给每个版本复制过一份测试实例，现在用不到了。
            let root = app.root.clone();
            std::thread::spawn(move || bench::cleanup_legacy(&root));
        }
        #[cfg(debug_assertions)]
        if let Ok(theme) = std::env::var("NAILONG_DEBUG_THEME") {
            app.prefs.theme = t::Preference::from_key(&theme);
        }
        // 调试钩子：跳过点击直接进下载中心，方便无头冒烟测试（仅 debug 构建）。
        #[cfg(debug_assertions)]
        if let Ok(id) = std::env::var("NAILONG_DEBUG_SELECT")
            && let Some(row) = app.rows.iter().find(|row| row.id.contains(&id))
        {
            app.selected = Some(row.id.clone());
        }
        #[cfg(debug_assertions)]
        if let Some(screen) = std::env::var_os("NAILONG_DEBUG_SCREEN") {
            let screen = screen.to_string_lossy().to_string();
            let row = app.selected_row().or_else(|| app.rows.first().cloned());
            match screen.as_str() {
                "install" => app.open_install(),
                "manage" => {
                    app.show_manage = true;
                    app.show_install = false;
                }
                "mods" => {
                    if row.is_some() {
                        app.content.row = row.clone();
                        app.open_mods();
                        // Preload the installed index so the tab count and
                        // "已装" badges match what the player sees after
                        // visiting the installed tab once.
                        app.start_mods_scan();
                    }
                }
                "saves" => {
                    if let Some(row) = row {
                        app.open_saves(row);
                    }
                }
                "installed" => {
                    if let Some(row) = row {
                        app.content.row = Some(row);
                        app.open_mods();
                        app.content.show_installed = true;
                        app.start_mods_scan();
                    }
                }
                "account" => {
                    app.account = Some(auth::Account {
                        name: "Steve".into(),
                        uuid: "069a79f444e94726a5befca90e38aaf5".into(),
                        access_token: "debug".into(),
                        refresh_token: "debug".into(),
                        expires_at: u64::MAX / 2,
                        xuid: None,
                        client_id: String::new(),
                    });
                    app.login.open = true;
                }
                "login" => {
                    app.login.url = Some(
                        "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
                    );
                    app.login.busy = true;
                }
                "login_error" => {
                    app.login.open = true;
                    app.login.error = Some("登录参数不完整，请更新到最新版本后重试".into());
                }
                "settings" => {
                    app.show_settings = true;
                }
                "log" => {
                    if let Some(row) = row {
                        app.play.last = Some(log::LastLaunch {
                            label: row.label.clone(),
                            log: launch::log_path(&app.root, &row.profile),
                            game_dir: row
                                .game_dir
                                .clone()
                                .unwrap_or_else(|| app.root.dir().to_path_buf()),
                            started: std::time::SystemTime::now() - Duration::from_secs(180),
                            outcome: log::Outcome::Exited(1),
                        });
                        app.error =
                            Some("游戏异常退出（代码 1）：缺少前置 Mod：日志里写了缺哪个".into());
                        app.open_log();
                    }
                }
                "java" => app.start_java_download(17, None, None),
                "update" => {
                    app.update.latest = Some(nailong_core::update::Release {
                        version: "0.39.0".into(),
                        notes: "下载页显示正在下什么、从哪下、多快、还要多久\n\n\
                                * 没有合适的 Java 时自动下载官方运行时，下完接着启动\n\
                                * 启动器里提示新版本，一键更新\n\
                                * 界面代码按页面和弹窗拆成模块"
                            .into(),
                        page: "https://github.com/dkjsiogu/cairn/releases".into(),
                        assets: Vec::new(),
                    });
                    app.update.open = true;
                    // 别让启动时的自动检查把这个假版本冲掉。
                    app.prefs.update_checked = u64::MAX / 2;
                }
                "running" => {
                    app.busy = Some(Task::Launch);
                    app.play.started = Some(Instant::now());
                }
                "progress" => {
                    app.busy = Some(Task::Install);
                    let mut activity = Activity::new("1.21.1 · NeoForge 21.1.264", "解析版本");
                    activity.update(Progress::stage("下载依赖库"));
                    for (i, name) in [
                        "minecraft/sounds/block/stone/step3.ogg",
                        "minecraft/textures/block/deepslate_bricks.png",
                        "minecraft/lang/zh_cn.json",
                        "minecraft/sounds/ambient/cave/cave13.ogg",
                        "icons/minecraft.icns",
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        activity.update(Progress {
                            stage: "下载游戏资源".into(),
                            files_done: 2808 + i as u64,
                            files_total: 3911,
                            bytes_done: 180_000_000 + i as u64 * 1_000_000,
                            bytes_total: 320_000_000,
                            current: Some(name.into()),
                            source: Some("resources.download.minecraft.net".into()),
                            ..Default::default()
                        });
                    }
                    app.activity = Some(activity);
                }
                _ => {}
            }
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_OPEN_MODS").is_some()
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            app.content.row = Some(row);
            app.open_mods();
        }
        #[cfg(debug_assertions)]
        if let Ok(term) = std::env::var("NAILONG_DEBUG_PROJECT")
            && app.show_mods
            && app.content.selected.is_none()
            && let Ok(page) = mods::search(
                &term,
                None,
                app.content.mc.as_deref(),
                app.content.loader,
                app.content.kind,
                0,
                8,
            )
            && let Some(project) = page.hits.into_iter().next()
        {
            app.request_icons(std::iter::once(project.clone()));
            app.select_mods_project(project);
        }
        #[cfg(debug_assertions)]
        if let Ok(term) = std::env::var("NAILONG_DEBUG_SEARCH")
            && app.show_mods
        {
            app.content.query = term;
            app.start_mods_search();
        }
        #[cfg(debug_assertions)]
        if std::env::var("NAILONG_DEBUG_REVIEW").as_deref() == Ok("quick")
            && app.show_mods
            && let Some(project) = app.content.selected.clone()
        {
            app.review_quick_install(project);
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_OPEN_SAVES").is_some()
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            app.open_saves(row);
        }
        #[cfg(debug_assertions)]
        if let Some(count) = std::env::var("NAILONG_DEBUG_UPDATES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            && let Some(row) = app.selected_row().or_else(|| app.rows.first().cloned())
        {
            let dir = row
                .game_dir
                .clone()
                .unwrap_or_else(|| app.root.version_dir(&row.profile));
            app.content.updates = installed::load(&dir)
                .entries
                .into_iter()
                .filter(|entry| entry.kind == mods::ContentKind::Mod)
                .take(count)
                .map(|entry| installed::UpdateCandidate {
                    latest: mods::Version {
                        source: mods::Source::Modrinth,
                        kind: mods::ContentKind::Mod,
                        project_id: String::new(),
                        id: String::new(),
                        name: entry.name.clone(),
                        version_number: "debug".into(),
                        game_versions: Vec::new(),
                        loaders: Vec::new(),
                        file_name: entry.file.clone(),
                        download_url: None,
                        sha1: None,
                        size: None,
                        published: None,
                        dependencies: Vec::new(),
                    },
                    entry,
                })
                .collect();
            app.content.row = Some(row);
        }
        if app.visible_count() == 0 && !app.rows.is_empty() {
            app.show_manage = true;
        }
        #[cfg(debug_assertions)]
        if std::env::var("NAILONG_DEBUG_SCREEN").as_deref() == Ok("welcome") {
            app.show_install = false;
            app.show_manage = false;
            app.show_mods = false;
            app.show_saves = false;
        }
        app
    }

    /// 安装和下载会占住进度页；游戏运行不会，玩的时候照样能看内容和存档。
    pub(crate) fn page_locked(&self) -> bool {
        self.busy.is_some_and(Task::shows_progress) || self.login.busy
    }

    /// 现在不能改这个版本的原因；`None` 表示可以改。
    pub(crate) fn write_block(&self, row_id: Option<&str>) -> Option<&'static str> {
        match self.busy {
            Some(Task::Install) | Some(Task::Mods) => Some("正在安装，完成后再试"),
            Some(Task::Bench) => Some("正在测试这台电脑，完成后再试"),
            Some(Task::Java) => Some("正在下载 Java，完成后再试"),
            Some(Task::Launch)
                if row_id.is_some() && self.play.running_row.as_deref() == row_id =>
            {
                Some("游戏正在使用这个版本，退出游戏后再修改")
            }
            _ => None,
        }
    }

    /// 页面底部的一行提示：错误、完成通知，以及“查看日志”。
    pub(crate) fn message_bar(&mut self, ui: &mut egui::Ui) {
        let (text, color, is_error) = match (&self.error, &self.notice) {
            (Some(error), _) => (error.clone(), t::danger(), true),
            (None, Some(notice)) => (notice.clone(), t::ok(), false),
            (None, None) => return,
        };
        let full = ui.max_rect();
        let bar = egui::Rect::from_min_max(
            egui::pos2(full.left(), full.bottom() - 40.0),
            full.right_bottom(),
        );
        ui.painter().rect_filled(bar, 0.0, t::surface());
        ui.painter().line_segment(
            [bar.left_top(), bar.right_top()],
            egui::Stroke::new(1.0, t::hairline()),
        );
        let mut close = false;
        let mut show_log = false;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(egui::vec2(24.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(color)).truncate());
        child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("×").size(15.0)).frame(false))
                .on_hover_text("关闭提示")
                .clicked()
            {
                close = true;
            }
            if is_error
                && ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("查看日志").size(13.0).color(t::sub()),
                        )
                        .frame(false),
                    )
                    .clicked()
            {
                show_log = true;
            }
        });
        if close {
            self.error = None;
            self.notice = None;
        }
        if show_log {
            self.open_log();
        }
    }

    /// 按钮不可用时给出的说明。
    pub(crate) fn busy_reason(&self) -> &'static str {
        match self.busy {
            Some(Task::Launch) => "游戏运行中，退出游戏后再安装",
            Some(Task::Bench) => "正在测试这台电脑，完成后再试",
            Some(Task::Java) => "正在下载 Java，完成后再试",
            _ => "正在安装，完成后再试",
        }
    }

    pub(crate) fn page_id(&self) -> u8 {
        if self.busy.is_some_and(Task::shows_progress) {
            return 10;
        }
        if self.show_mods {
            if self.content.show_installed {
                6
            } else if self.content.selected.is_some() {
                5
            } else if self.content.searching {
                4
            } else {
                3
            }
        } else if self.show_saves {
            8
        } else if self.show_manage {
            2
        } else if self.show_install {
            1
        } else {
            0
        }
    }

    /// 全局快捷键：只在不输入文字、没有弹窗时生效。
    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input()
            || self.show_settings
            || self.login.open
            || self.login.busy
            || self.login.url.is_some()
            || self.pending_install.is_some()
            || self.rollback.open
            || self.log_view.is_some()
            || self.launch_edit.is_some()
            || self
                .home
                .shots
                .as_ref()
                .is_some_and(|shots| shots.viewing())
        {
            return;
        }
        let mut nav = None;
        let mut settings = false;
        let mut refresh = false;
        let mut search = false;
        let mut logs = false;
        ctx.input_mut(|input| {
            for (key, target) in [
                (egui::Key::Num1, Nav::Home),
                (egui::Key::Num2, Nav::Versions),
                (egui::Key::Num3, Nav::Content),
                (egui::Key::Num4, Nav::Saves),
            ] {
                if input.consume_key(egui::Modifiers::CTRL, key) {
                    nav = Some(target);
                }
            }
            settings = input.consume_key(egui::Modifiers::CTRL, egui::Key::Comma);
            refresh = input.consume_key(egui::Modifiers::NONE, egui::Key::F5);
            search = input.consume_key(egui::Modifiers::CTRL, egui::Key::F);
            logs = input.consume_key(egui::Modifiers::CTRL, egui::Key::L);
        });
        if logs {
            self.open_log();
            return;
        }
        if search && self.show_mods {
            self.content.focus_search = true;
        }
        if settings {
            self.login.open = false;
            self.show_settings = true;
        }
        if refresh {
            self.refresh();
        }
        if let Some(nav) = nav {
            self.nav_go(nav);
        }
    }

    pub(crate) fn refresh_java(&mut self) {
        let runtimes = java::find_all(&self.root);
        self.env.java_ok = runtimes.iter().any(|runtime| runtime.major >= 21);
        self.env.java_label = runtimes
            .iter()
            .find(|runtime| runtime.major >= 21)
            .map(|runtime| runtime.label());
        self.env.javas = runtimes;
        self.play.tuning_cache.clear();
    }

    pub(crate) fn refresh(&mut self) {
        self.home.cache = None;
        self.home.verified = None;
        self.play.tuning_cache.clear();
        self.version_cache = None;
        let required_by = profile_dependents(&self.root);
        let used_by = instance_usage(&self.root);
        let mut rows = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.root.versions()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().to_string();
                if !self.root.version_json(&id).exists() {
                    continue;
                }
                let (mc, loader, loader_version) = profile_info(&self.root, &id);
                let label = match &loader_version {
                    Some(version) => format!("{mc} · {} {version}", loader.name()),
                    None => mc.clone(),
                };
                let row_health = health::inspect(&self.root, &id);
                let required = required_by.get(&id).copied().unwrap_or(0);
                let used = used_by.get(&id).copied().unwrap_or(0);
                let isolated = self.root.isolated_marker(&id).exists()
                    || self.root.version_dir(&id).join("mods").is_dir();
                let game_dir = isolated.then(|| self.root.version_dir(&id));
                rows.push(Row {
                    id: id.clone(),
                    profile: id,
                    label,
                    mc,
                    loader,
                    loader_version,
                    game_dir,
                    instance: None,
                    hidden: loader == LoaderKind::Vanilla && required > 0,
                    required_by: required,
                    used_by: used,
                    health: row_health,
                });
            }
        }
        if let Ok(entries) = std::fs::read_dir(self.root.instances()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let meta_path = entry.path().join("instance.json");
                let Ok(bytes) = std::fs::read(&meta_path) else {
                    continue;
                };
                let Ok(meta) = serde_json::from_slice::<modpack::InstanceMeta>(&bytes) else {
                    continue;
                };
                if !self.root.version_json(&meta.profile).exists() {
                    continue;
                }
                let (mc, loader, loader_version) = profile_info(&self.root, &meta.profile);
                let game_dir = entry.path().join(&meta.game_dir);
                let id = format!("instance:{}", entry.file_name().to_string_lossy());
                rows.push(Row {
                    id,
                    profile: meta.profile.clone(),
                    label: format!("{} · 整合包", meta.name),
                    mc,
                    loader,
                    loader_version,
                    game_dir: Some(game_dir),
                    instance: Some(meta.name),
                    hidden: false,
                    required_by: 0,
                    used_by: 0,
                    health: health::inspect(&self.root, &meta.profile),
                });
            }
        }
        rows.sort_by(|a, b| {
            b.instance
                .is_some()
                .cmp(&a.instance.is_some())
                .then_with(|| mc_key(&b.mc).cmp(&mc_key(&a.mc)))
                .then_with(|| a.loader.rank().cmp(&b.loader.rank()))
        });
        self.rows = rows;
        self.ensure_visible_selection();
    }

    pub(crate) fn selected_row(&self) -> Option<Row> {
        self.selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
            .cloned()
    }

    pub(crate) fn is_visible(&self, row: &Row) -> bool {
        self.show_base || !row.hidden
    }

    pub(crate) fn visible_count(&self) -> usize {
        self.rows.iter().filter(|r| self.is_visible(r)).count()
    }

    pub(crate) fn visible_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| self.is_visible(r))
    }

    pub(crate) fn ensure_visible_selection(&mut self) {
        let valid = self
            .selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
            .is_some_and(|r| self.is_visible(r));
        if !valid {
            // 首屏优先选“看起来像常用版本”的那个：有整合包实例、有内容、
            // 安装完整。没有任何线索时退回列表最后一个，避免总选到空版本。
            self.selected = self
                .rows
                .iter()
                .filter(|row| self.is_visible(row))
                .max_by_key(|row| {
                    let dir = row
                        .game_dir
                        .clone()
                        .unwrap_or_else(|| self.root.version_dir(&row.profile));
                    let content = installed::load(&dir).entries.len();
                    (row.instance.is_some(), content, row.health.missing == 0)
                })
                .map(|row| row.id.clone());
        }
    }

    pub(crate) fn save_ui_config(&self) {
        let value = serde_json::json!({
            "username": self.prefs.username,
            "memory": self.prefs.memory,
            "gc": self.prefs.gc.key(),
            "jvm_args": self.prefs.jvm_args.trim(),
            "reduce_motion": self.prefs.reduce_motion,
            "theme": self.prefs.theme.key(),
            "update_checked": self.prefs.update_checked,
            "update_skip": self.prefs.update_skip,
            "source": self.prefs.source,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
            let _ = std::fs::write(self.root.dir().join("ui.json"), bytes);
        }
    }
}

pub(crate) struct App {
    // ---- 数据和选择 ----
    /// 数据目录。
    pub(crate) root: Root,
    /// 左侧的版本列表（原版、加载器和整合包）。
    pub(crate) rows: Vec<Row>,
    /// 当前选中的版本行。
    pub(crate) selected: Option<String>,
    /// 登录的微软账户；None 是离线账户。
    pub(crate) account: Option<auth::Account>,
    /// 玩家名、全局启动参数和外观，存在数据目录的 `ui.json`。
    pub(crate) prefs: UiPrefs,
    // ---- 导航：哪个页面在前面 ----
    pub(crate) show_install: bool,
    pub(crate) show_manage: bool,
    pub(crate) show_mods: bool,
    pub(crate) show_saves: bool,
    /// 版本页是否也列出被加载器依赖的原版。
    pub(crate) show_base: bool,
    /// 等待确认删除的版本。
    pub(crate) delete_confirm: Option<String>,
    /// 当前页面的编号，变了就放一次切页动效。
    pub(crate) page_key: u8,
    /// 进入当前页面的时间。
    pub(crate) page_enter_at: f64,
    /// 窗口上一帧是否在前台：回到窗口时重新核对内容目录。
    pub(crate) window_focused: bool,
    // ---- 各页面和弹窗自己的状态 ----
    /// 启动台：摘要缓存、后台核对和截图墙。
    pub(crate) home: HomeState,
    /// 安装新版本的表单和版本目录。
    pub(crate) install: InstallForm,
    /// 内容页：发现和已安装两种视图共用的状态。
    pub(crate) content: ContentState,
    /// 存档页：世界、快照和自动备份。
    pub(crate) worlds: WorldsState,
    /// 账户弹窗和正在进行的微软登录。
    pub(crate) login: LoginState,
    /// 恢复点弹窗。
    pub(crate) rollback: RollbackState,
    /// 启动器自己的更新：检查结果、更新弹窗和下载进度。
    pub(crate) update: UpdateState,
    /// 等待确认的安装。
    pub(crate) pending_install: Option<PendingInstall>,
    /// 版本页摘要（Mod、资源包、光影、存档数）的缓存。
    pub(crate) version_cache: Option<(String, (usize, usize, usize, usize))>,
    /// 设置弹窗。
    pub(crate) show_settings: bool,
    /// 设置里的「查看计算」展开了没有。
    pub(crate) show_tuning: bool,
    /// 实测调参的确认弹窗。
    pub(crate) bench_confirm: bool,
    /// 启动参数窗口。
    pub(crate) launch_edit: Option<LaunchEdit>,
    /// 日志窗口；None 时关着。
    pub(crate) log_view: Option<log::LogView>,
    // ---- 开始游戏和这台电脑 ----
    /// 开始游戏：每个版本的启动参数、算好的调参，以及最近一次启动。
    pub(crate) play: PlayState,
    /// 这台电脑：找到的 Java、配置，以及「测一下这台电脑」的结论。
    pub(crate) env: Env,
    // ---- 后台任务和提示 ----
    /// 正在跑的后台任务；同一时间只有一个。
    pub(crate) busy: Option<Task>,
    /// 任务的「取消」已经按过。
    pub(crate) cancel_requested: bool,
    /// 正在跑的任务给人看的样子：步骤、进度、刚下载的文件、速度。
    pub(crate) activity: Option<Activity>,
    /// 后台任务的结果和进度从这里回来。
    pub(crate) rx: Option<mpsc::Receiver<Msg>>,
    /// 页面底部的错误。
    pub(crate) error: Option<String>,
    /// 页面底部的提示。
    pub(crate) notice: Option<String>,
    /// 启动器自己的记录，日志窗口的「启动器」标签显示它。
    pub(crate) log: Vec<String>,
    /// 当前弹窗的编号。
    pub(crate) modal_key: u8,
    /// 弹窗打开的时间。
    pub(crate) modal_started: f64,
}

pub(crate) struct UiPrefs {
    pub(crate) username: String,
    pub(crate) memory: Option<u32>,
    pub(crate) gc: tuning::GcChoice,
    pub(crate) jvm_args: String,
    pub(crate) reduce_motion: bool,
    pub(crate) theme: t::Preference,
    /// 上次检查更新的时间（Unix 秒）。
    pub(crate) update_checked: u64,
    /// 玩家选了「跳过这个版本」的版本号。
    pub(crate) update_skip: String,
    /// 下载源：`official` 官方优先（连不上自动换镜像），`bmcl` 镜像优先。
    pub(crate) source: String,
}

/// 玩家名、内存和外观存在数据目录的 `ui.json`，随设置弹窗关闭落盘。
pub(crate) fn load_ui_prefs(root: &Root) -> UiPrefs {
    let value = std::fs::read(root.dir().join("ui.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let get = |key: &str| value.as_ref().and_then(|value| value.get(key));
    let username = get("username")
        .and_then(|value| value.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Player")
        .to_string();
    // 0.33 及以前没有 "gc"，那时的 4096 是默认值而不是用户选的，迁移成自动。
    let legacy = get("gc").is_none();
    let memory = get("memory")
        .and_then(|value| value.as_u64())
        .filter(|&mb| !(legacy && mb == 4096))
        .map(|mb| mb.clamp(1024, 16384) as u32);
    let gc = get("gc")
        .and_then(|value| value.as_str())
        .map(tuning::GcChoice::from_key)
        .unwrap_or_default();
    let jvm_args = get("jvm_args")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let reduce_motion = get("reduce_motion")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let theme = get("theme")
        .and_then(|value| value.as_str())
        .map(t::Preference::from_key)
        .unwrap_or(t::Preference::System);
    UiPrefs {
        username,
        memory,
        gc,
        jvm_args,
        reduce_motion,
        theme,
        update_checked: get("update_checked")
            .and_then(|value| value.as_u64())
            .unwrap_or(0),
        update_skip: get("update_skip")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        source: match get("source").and_then(|value| value.as_str()) {
            Some("bmcl") => "bmcl".into(),
            _ => "official".into(),
        },
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        t::sync(ui.ctx());
        self.poll();
        self.poll_update(ui.ctx());
        motion::set_reduced(self.prefs.reduce_motion);
        self.handle_shortcuts(ui.ctx());
        self.drain_icons(ui.ctx());
        if self.worlds.backup.enabled {
            ui.ctx().request_repaint_after(Duration::from_secs(5));
            self.maybe_auto_backup();
        }
        let dropped: Vec<PathBuf> = ui
            .ctx()
            .input_mut(|i| std::mem::take(&mut i.raw.dropped_files))
            .into_iter()
            .map(|file| file.path().to_path_buf())
            .collect();
        // 安装页有自己的整合包拖放区，那里只认整合包。
        if !dropped.is_empty() {
            self.content.drop_preview = None;
            let plan = self.plan_drop(&dropped);
            if self.show_install && plan.items.is_empty() && plan.packs.len() == 1 {
                if self.busy.is_none() {
                    self.start_modpack(plan.packs[0].clone());
                }
            } else {
                self.import_dropped(plan);
            }
        }
        // 回到窗口时，文件管理器里做的改动直接反映出来，不用点「重新扫描」。
        let focused = ui.ctx().input(|i| i.focused);
        if focused && !self.window_focused {
            self.rescan_if_changed();
        }
        self.window_focused = focused;
        if self.busy.is_some()
            || self.login.busy
            || self.worlds.rx.is_some()
            || self.content.rx.is_some()
            || self.content.scan_rx.is_some()
            || self.rollback.rx.is_some()
            || self.content.browse_loading
            || !self.content.icons_pending.is_empty()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let now = ui.ctx().input(|input| input.time);
        let page = self.page_id();
        if page != self.page_key {
            self.page_key = page;
            // 别的页面可能改了内容或快照，回到启动台时重新读取。
            self.home.cache = None;
            self.home.verified = None;
            self.play.tuning_cache.clear();
            self.page_enter_at = now;
        }
        let enter = if self.prefs.reduce_motion {
            1.0
        } else {
            (((now - self.page_enter_at) / 0.15) as f32).clamp(0.0, 1.0)
        };
        let eased = motion::ease_out_cubic(enter);
        if enter < 1.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("NAILONG_DEBUG_HOVER_CYCLE").is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        self.sidebar(ui);
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(t::bg()))
            .show(ui, |ui| {
                ui.add_space((1.0 - eased) * 6.0);
                if self.busy.is_some_and(Task::shows_progress) {
                    self.progress_view(ui);
                } else if self.show_mods {
                    self.mods_view(ui);
                } else if self.show_saves {
                    self.saves_view(ui);
                } else if self.show_manage {
                    self.manage_view(ui);
                } else if self.show_install {
                    self.install_view(ui);
                } else {
                    self.main_view(ui);
                }
                // 版本、内容、存档页没有放全局提示的位置，统一放在底部。
                if matches!(page, 2..=8) {
                    self.message_bar(ui);
                }
                self.drop_overlay(ui);
                // 用一层底色做淡入：嵌套面板各有画笔，逐个调透明度并不可靠。
                let veil = 1.0 - eased;
                if veil > 0.001 {
                    ui.painter()
                        .rect_filled(ui.max_rect(), 0.0, t::tint(t::bg(), veil));
                }
            });
        self.account_overlay(ui.ctx());
        self.settings_overlay(ui.ctx());
        self.bench_overlay(ui.ctx());
        self.rollback_overlay(ui.ctx());
        self.install_review_overlay(ui.ctx());
        self.log_overlay(ui.ctx());
        self.launch_overlay(ui.ctx());
        self.update_overlay(ui.ctx());
        if let Some(shots) = &mut self.home.shots {
            shots.viewer(ui.ctx());
        }
        motion::focus_ring(ui.ctx());
    }
}
