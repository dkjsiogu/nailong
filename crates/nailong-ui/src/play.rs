//! 开始游戏：合并全局和这个版本的启动参数、选 Java、算 JVM 参数，然后在后台启动。

use crate::prelude::*;

impl App {
    /// 这个版本单独的启动参数；没设置过时全部跟随全局。
    pub(crate) fn version_cfg(&self, row_id: &str) -> VersionLaunch {
        self.play.versions.get(row_id).cloned().unwrap_or_default()
    }

    /// 全局设置叠上这个版本自己的设置。
    pub(crate) fn tuning_settings(&self, cfg: &VersionLaunch) -> tuning::Settings {
        tuning::Settings {
            memory_mb: cfg.memory.map_or(self.prefs.memory, MemoryPick::memory_mb),
            min_memory_mb: None,
            gc: cfg.gc.unwrap_or(self.prefs.gc),
            extra: tuning::split_args(cfg.jvm_args.as_deref().unwrap_or(&self.prefs.jvm_args)),
            ..Default::default()
        }
    }

    /// 指定路径的 Java：先在检测到的里找，找不到再读一次它的版本。
    pub(crate) fn java_at(&mut self, path: &std::path::Path) -> Option<java::JavaRuntime> {
        let same = |runtime: &java::JavaRuntime| {
            runtime.path == path
                || std::fs::canonicalize(&runtime.path).ok() == std::fs::canonicalize(path).ok()
        };
        if let Some(found) = self
            .env
            .javas
            .iter()
            .chain(&self.env.extra_javas)
            .find(|r| same(r))
        {
            return Some(found.clone());
        }
        let runtime = java::probe(path).ok()?;
        self.env.extra_javas.push(runtime.clone());
        Some(runtime)
    }

    /// 这个版本启动时会用的堆和 GC 参数，和 `launch` 的算法相同；只在缓存失效时读盘。
    pub(crate) fn tuning_for(&mut self, row: &Row) -> Option<tuning::Plan> {
        let cfg = self.version_cfg(&row.id);
        self.plan_for(row, &cfg).map(|(plan, _)| plan)
    }

    /// 缓存按「版本 + 指定的 Java」区分：启动参数窗口里换 Java 时不会串。
    pub(crate) fn tuning_key(row_id: &str, cfg: &VersionLaunch) -> String {
        format!("{row_id}\u{1f}{:?}", cfg.java)
    }

    /// 按给定的启动参数（可以是还没保存的草稿）算出会用的 JVM 参数和 Java。
    pub(crate) fn plan_for(
        &mut self,
        row: &Row,
        cfg: &VersionLaunch,
    ) -> Option<(tuning::Plan, java::JavaRuntime)> {
        let key = Self::tuning_key(&row.id, cfg);
        if !self.play.tuning_cache.contains_key(&key) {
            let game_dir = row
                .game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf());
            let chosen = cfg.java.as_deref().and_then(|path| self.java_at(path));
            let entry = if cfg.java.is_some() && chosen.is_none() {
                None
            } else {
                tuning::workload(
                    &self.root,
                    &row.profile,
                    &game_dir,
                    &self.env.javas,
                    chosen.as_ref(),
                )
                .ok()
                .map(|(work, java)| {
                    let learned = learned::load(&self.root, &row.profile, &game_dir);
                    (work, java, learned)
                })
            };
            self.env.machine = tuning::Machine::detect();
            self.play.tuning_cache.insert(key.clone(), entry);
        }
        let (work, java, learned) = self.play.tuning_cache.get(&key)?.as_ref()?;
        let settings = tuning::Settings {
            bench: self
                .env
                .bench
                .as_ref()
                .and_then(|store| store.verdict(java)),
            learned: learned.clone(),
            ..self.tuning_settings(cfg)
        };
        Some((
            tuning::plan(&self.env.machine, work, &settings),
            java.clone(),
        ))
    }

    /// 缓存里这个版本当前设置对应的那一条。
    pub(crate) fn tuning_entry(&self, row_id: &str) -> Option<&TuningEntry> {
        let key = Self::tuning_key(row_id, &self.version_cfg(row_id));
        self.play.tuning_cache.get(&key)?.as_ref()
    }

    /// 例如「内存 5 GB（自动）」。
    pub(crate) fn memory_label(&mut self, row: &Row) -> String {
        match self.tuning_for(row) {
            Some(plan) if plan.auto_memory => format!("内存 {}（自动）", tuning::gb(plan.heap_mb)),
            Some(plan) => format!("内存 {}", tuning::gb(plan.heap_mb)),
            None => match self.prefs.memory {
                Some(mb) => format!("内存 {}", tuning::gb(mb)),
                None => "内存自动".into(),
            },
        }
    }

    pub(crate) fn start_launch(&mut self) {
        if self.busy.is_some() {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.health.missing > 0 {
            self.error = Some("这个版本缺少文件，先补全再启动".into());
            return;
        }
        let cfg = self.version_cfg(&row.id);
        let required = launch::required_java_for(&self.root, &row.profile).unwrap_or(8);
        match &cfg.java {
            Some(path) => match self.java_at(path) {
                Some(java) if java.major < required => {
                    self.error = Some(format!(
                        "这个版本需要 Java {required}，启动参数里指定的是 Java {}；在启动参数里换一个",
                        java.major
                    ));
                    return;
                }
                Some(_) => {}
                None => {
                    self.error = Some(format!(
                        "启动参数里指定的 Java 不见了：{}；在启动参数里换一个",
                        path.display()
                    ));
                    return;
                }
            },
            None => {
                if !self.env.javas.iter().any(|java| java.major >= required) {
                    self.refresh_java();
                }
                if !self.env.javas.iter().any(|java| java.major >= required) {
                    // 没有合适的 Java：直接下载官方的，下完接着启动。玩家不用自己去找。
                    let component = launch::java_requirement(&self.root, &row.profile)
                        .ok()
                        .and_then(|(_, component)| component);
                    self.start_java_download(required, component, Some(row.id.clone()));
                    return;
                }
            }
        }
        self.error = None;
        self.notice = None;
        self.busy = Some(Task::Launch);
        self.play.started = Some(Instant::now());
        self.play.running_row = Some(row.id.clone());
        self.log.push(format!("启动 {}", row.id));
        self.play.last = Some(log::LastLaunch {
            label: row.label.clone(),
            log: launch::log_path(&self.root, &row.profile),
            game_dir: row
                .game_dir
                .clone()
                .unwrap_or_else(|| self.root.dir().to_path_buf()),
            started: std::time::SystemTime::now(),
            outcome: log::Outcome::Running,
        });
        self.play
            .tuning_cache
            .retain(|key, _| !key.starts_with(&format!("{}\u{1f}", row.id)));
        if let Some(plan) = self.tuning_for(&row) {
            self.log
                .push(format!("JVM {}，{}", plan.headline(), plan.basis));
            self.log
                .extend(plan.warnings.iter().map(|w| format!("注意：{w}")));
        }
        let root = self.root.clone();
        let tuning = self.tuning_settings(&cfg);
        let [width, height] = cfg.window();
        let java_path = cfg.java.clone();
        let account_id = self.accounts.current.clone();
        let fallback_name = self.prefs.username.clone();
        let version_label = row.label.clone();
        let embed_shots = self.prefs.embed_shots;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            use nailong_core::accounts::{self, Ready};
            let identity = match &account_id {
                Some(id) => match accounts::ready(&root, id) {
                    Ok(Ready::Go(identity)) => identity,
                    Ok(Ready::Relogin { id, reason }) => {
                        let _ = tx.send(Msg::NeedRelogin(id, reason));
                        return;
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::LaunchFailed(e.to_string()));
                        return;
                    }
                },
                None => accounts::Identity {
                    uuid: launch::offline_uuid(&fallback_name),
                    name: fallback_name,
                    access_token: None,
                    xuid: None,
                    client_id: None,
                    user_type: "legacy",
                    external: None,
                    kind: "离线".into(),
                },
            };
            let injector = match &identity.external {
                Some(external) => match nailong_core::yggdrasil::injector_jar(&root) {
                    Ok(jar) => Some(launch::Injector {
                        jar,
                        api: external.api.clone(),
                        metadata: external.metadata.clone(),
                    }),
                    Err(e) => {
                        let _ =
                            tx.send(Msg::LaunchFailed(format!("authlib-injector 下载失败：{e}")));
                        return;
                    }
                },
                None => None,
            };
            let opts = LaunchOptions {
                username: identity.name.clone(),
                uuid: Some(identity.uuid.clone()),
                access_token: identity.access_token.clone(),
                xuid: identity.xuid.clone(),
                client_id: identity.client_id.clone(),
                user_type: Some(identity.user_type.to_string()),
                injector,
                shots: Some(nailong_core::shots::Tag {
                    version: version_label,
                    player: identity.name.clone(),
                    player_uuid: identity.uuid.clone(),
                    account: identity.kind.clone(),
                    embed: embed_shots,
                }),
                memory_mb: tuning.memory_mb,
                gc: tuning.gc,
                jvm_args: tuning.extra,
                game_dir: row.game_dir.clone(),
                java: java_path,
                width,
                height,
                ..Default::default()
            };
            let msg = match launch::launch(&root, &row.profile, &opts) {
                Ok(0) => Msg::LaunchDone(0, None),
                Ok(code) => {
                    // 在后台线程里读日志找原因，界面不用等。
                    let reason = std::fs::read(launch::log_path(&root, &row.profile))
                        .ok()
                        .and_then(|bytes| {
                            let tail = &bytes[bytes.len().saturating_sub(2 << 20)..];
                            launch::diagnose(&String::from_utf8_lossy(tail))
                        });
                    Msg::LaunchDone(code, reason)
                }
                Err(e) => Msg::LaunchFailed(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }
}

/// 开始游戏：每个版本的启动参数、算好的调参，以及最近一次启动。
pub(crate) struct PlayState {
    /// 每个版本的负载（Java、加载器、Mod 数）和实测记录（以及它是否仍然有效），
    /// 进页面和内容变化时重算。
    pub(crate) tuning_cache: HashMap<String, Option<TuningEntry>>,
    /// 每个版本单独的启动参数，键是版本行的 id。
    pub(crate) versions: BTreeMap<String, VersionLaunch>,
    /// 最近一次启动的日志在哪、结果如何。
    pub(crate) last: Option<log::LastLaunch>,
    /// 这次启动游戏的开始时间，用来显示已运行时长。
    pub(crate) started: Option<Instant>,
    /// 正在运行的游戏用的是哪个版本；运行期间这个版本的内容和存档不能改。
    pub(crate) running_row: Option<String>,
    /// Java 下完后接着启动的版本行。
    pub(crate) launch_after_java: Option<String>,
}

impl App {
    /// 下载官方的 Java 运行时。`then_launch` 是下完后要启动的版本行。
    pub(crate) fn start_java_download(
        &mut self,
        required: u32,
        component: Option<String>,
        then_launch: Option<String>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.busy = Some(Task::Java);
        self.cancel_requested = false;
        cancel::reset();
        self.play.launch_after_java = then_launch;
        self.activity = Some(Activity::new(
            format!("Java {required} · 官方运行时"),
            "查找官方的 Java",
        ));
        self.log.push(format!("下载 Java {required}"));
        let root = self.root.clone();
        let source = self.prefs.source.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let progress = tx.clone();
            let sink = from_fn(move |p| {
                let _ = progress.send(Msg::Progress(p));
            });
            let result = nailong_core::http::Http::new(&source).and_then(|http| {
                let offer = nailong_core::runtime::find(&http, required, component.as_deref())?;
                nailong_core::runtime::install(&root, &http, &offer, &sink)
                    .map(|_| format!("Java {}（官方运行时 {}）", offer.major, offer.version))
            });
            let msg = match result {
                Ok(label) => Msg::JavaDone(Ok(label)),
                Err(Error::Cancelled) => Msg::JavaCancelled,
                Err(e) => Msg::JavaDone(Err(e.to_string())),
            };
            let _ = tx.send(msg);
        });
    }

    /// Java 下完：刷新 Java 列表；是从「开始游戏」来的就接着启动。空的错误表示取消了。
    pub(crate) fn finish_java_download(&mut self, result: Result<String, String>) {
        self.busy = None;
        self.rx = None;
        self.activity = None;
        self.cancel_requested = false;
        cancel::reset();
        let then = self.play.launch_after_java.take();
        match result {
            Ok(label) => {
                self.refresh_java();
                self.play.tuning_cache.clear();
                self.log.push(format!("已装好 {label}"));
                self.notice = Some(format!("已装好 {label}"));
                if let Some(id) = then {
                    self.selected = Some(id);
                    self.start_launch();
                }
            }
            Err(e) if e.is_empty() => {
                self.notice = Some("已停止下载 Java；下载好的部分会留着，下次接着下".into());
            }
            Err(e) => {
                self.log.push(format!("Java 下载失败: {e}"));
                self.error = Some(format!("Java 没下载下来：{e}"));
            }
        }
    }
}
