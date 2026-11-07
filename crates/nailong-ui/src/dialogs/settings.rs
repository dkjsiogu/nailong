//! 设置弹窗：账户入口、玩家名、Java、性能（内存、回收器、测这台电脑）、目录和诊断。

use crate::prelude::*;

impl App {
    pub(crate) fn start_bench(&mut self) {
        if self.busy.is_some() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.busy = Some(Task::Bench);
        self.cancel_requested = false;
        cancel::reset();
        self.activity = Some(Activity::new("这台电脑", "准备测试"));
        self.log.push("测一下这台电脑".into());
        let root = self.root.clone();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let progress = tx.clone();
            let sink = from_fn(move |p| {
                let _ = progress.send(Msg::Progress(p));
            });
            let msg = match bench::run(&root, &sink) {
                Ok(store) => Msg::BenchDone(Ok(store)),
                Err(Error::Cancelled) => Msg::BenchCancelled,
                Err(e) => Msg::BenchDone(Err(e.to_string())),
            };
            let _ = tx.send(msg);
        });
    }

    pub(crate) fn finish_bench(&mut self) {
        self.busy = None;
        self.rx = None;
        self.activity = None;
        self.cancel_requested = false;
        cancel::reset();
        self.play.tuning_cache.clear();
    }

    /// 开始测试前说清楚会发生什么。
    pub(crate) fn bench_overlay(&mut self, ctx: &egui::Context) {
        if !self.bench_confirm {
            return;
        }
        let (picked, _) = bench::candidates(&self.env.javas);
        let can_run = !picked.is_empty();
        let mut start = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("bench_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.label(
                    egui::RichText::new("测一下这台电脑")
                        .size(24.0)
                        .family(display_family()),
                );
                ui.label(
                    egui::RichText::new(if can_run {
                        format!(
                            "用 {} 比较 G1 和 ZGC，结论所有版本共用",
                            picked
                                .iter()
                                .map(|java| format!("Java {}", java.major))
                                .collect::<Vec<_>>()
                                .join("、")
                        )
                    } else {
                        "这台电脑没有 64 位的 Java 21 或更新版本：只能用 G1，不需要测试".into()
                    })
                    .size(13.0)
                    .color(if can_run { t::sub() } else { t::warn() }),
                );
                ui.add_space(14.0);
                let minutes = (picked.len() as f32 * 1.4).ceil().max(1.0) as u32;
                for line in [
                    "不开游戏，也不碰任何版本：后台跑一个模拟游戏负载的小程序".to_string(),
                    "负载照游戏来：常驻数据、区块缓存、生成线程、服务端和不封顶的渲染".to_string(),
                    "比平均帧率、1% 低帧、能看出来的顿和 CPU，分常规和大型整合包两档".to_string(),
                    format!("大约 {minutes} 分钟，期间电脑会比较忙，可以随时停止"),
                    "内存大小不在这里测：正常玩一局，启动器会从游戏里学".to_string(),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("·").size(13.0).color(t::dim()));
                        ui.label(egui::RichText::new(line).size(13.0));
                    });
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if motion::primary_sized(ui, "开始测试", can_run, 120.0, 34.0, 13.0) {
                        start = true;
                    }
                    ui.add_space(8.0);
                    if motion::secondary_sized(ui, "取消", true, 72.0, 34.0, 13.0).clicked() {
                        close = true;
                    }
                });
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if start {
            self.bench_confirm = false;
            self.start_bench();
        } else if close {
            self.bench_confirm = false;
        }
    }

    /// 设置里的性能卡片：内存、回收器，以及当前版本会用的参数和计算过程。
    pub(crate) fn tuning_card(&mut self, ui: &mut egui::Ui) {
        let row = self.selected_row();
        let plan = row.as_ref().and_then(|row| self.tuning_for(row));
        egui::Frame::new()
            .fill(t::bg())
            .stroke(egui::Stroke::new(1.0, t::hairline()))
            .corner_radius(motion::R)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(442.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("内存").size(13.0).color(t::sub()));
                    let auto_mb = plan.as_ref().map_or(4096, |plan| {
                        if plan.auto_memory { plan.heap_mb } else { 4096 }
                    });
                    memory_stepper(ui, &mut self.prefs.memory, auto_mb);
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("回收器").size(13.0).color(t::sub()));
                    for gc in tuning::GcChoice::ALL {
                        if chip(ui, gc.label(), self.prefs.gc == gc).clicked() {
                            self.prefs.gc = gc;
                        }
                    }
                });
                let (Some(row), Some(plan)) = (row, plan) else {
                    return;
                };
                let entry = self
                    .tuning_entry(&row.id)
                    .map(|(work, java, learned)| (work.clone(), java.clone(), learned.clone()));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(plan.headline()).size(14.0));
                    // 版本名占剩下的地方，太长就省略，不和右边的链接重叠。
                    let room = (ui.available_width() - 72.0).max(40.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(room, 20.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&row.label).size(13.0).color(t::dim()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&row.label);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.show_tuning {
                            "收起"
                        } else {
                            "查看计算"
                        };
                        if action_link(ui, label).clicked() {
                            self.show_tuning = !self.show_tuning;
                        }
                    });
                });
                ui.label(egui::RichText::new(&plan.basis).size(13.0).color(t::dim()));
                // 这里是全局默认；这个版本自己改过的项以启动参数为准，说清楚免得以为没生效。
                let cfg = self.version_cfg(&row.id);
                if !cfg.is_default() {
                    let mut own = Vec::new();
                    if cfg.java.is_some() {
                        own.push("Java");
                    }
                    if cfg.memory.is_some() {
                        own.push("内存");
                    }
                    if cfg.gc.is_some() {
                        own.push("回收器");
                    }
                    if cfg.jvm_args.is_some() {
                        own.push("JVM 参数");
                    }
                    if cfg.window.is_some() {
                        own.push("窗口");
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "这个版本单独设置了{}，以启动参数为准",
                                own.join("、")
                            ))
                            .size(13.0)
                            .color(t::accent()),
                        );
                        if action_link(ui, "去看看").clicked() {
                            self.show_settings = false;
                            self.open_launch_edit(&row);
                        }
                    });
                }
                for warning in &plan.warnings {
                    ui.label(egui::RichText::new(warning).size(13.0).color(t::warn()));
                }
                // 内存从哪来：玩过就按实测，没玩过提示一句。
                if plan.auto_memory
                    && entry
                        .as_ref()
                        .is_none_or(|(_, _, learned)| learned.as_ref().is_none_or(|l| l.live_mb.is_none()))
                {
                    ui.label(
                        egui::RichText::new("内存现在按公式估算；正常玩一局后改按游戏里的实测")
                            .size(13.0)
                            .color(t::dim()),
                    );
                }
                // 回收器从哪来：这台电脑测过没有。
                let java = entry.as_ref().map(|(_, java, _)| java.clone());
                let (text, color, action) = self.bench_status(java.as_ref());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(text).size(13.0).color(color))
                            .truncate(),
                    );
                    if let Some(action) = action {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_link(ui, action).clicked() {
                                self.bench_confirm = true;
                            }
                        });
                    }
                });
                if !self.show_tuning {
                    if !self.prefs.jvm_args.trim().is_empty() {
                        ui.label(
                            egui::RichText::new("另有自定义 JVM 参数")
                                .size(13.0)
                                .color(t::dim()),
                        );
                    }
                    return;
                }
                ui.add_space(8.0);
                for step in &plan.steps {
                    ui.label(egui::RichText::new(step).size(13.0).color(t::sub()));
                }
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(plan.args().join(" "))
                            .size(13.0)
                            .monospace()
                            .color(t::sub()),
                    )
                    .wrap(),
                );
                if let Some(store) = &self.env.bench {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "这台电脑的测试（{}）",
                            bench::age(store.when)
                        ))
                        .size(13.0)
                        .color(t::sub()),
                    );
                    for result in &store.javas {
                        for tier in &result.tiers {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Java {} · {}档 → {}",
                                    result.major,
                                    tier.label,
                                    tier.collector.label()
                                ))
                                .size(13.0)
                                .color(t::sub()),
                            );
                            for run in [&tier.g1, &tier.zgc] {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "    {} {}：平均 {:.0} 帧 · 1% 低 {:.0} 帧 · 每分钟顿 {:.0} 次{}",
                                        run.collector.label(),
                                        tuning::gb(run.heap_mb),
                                        run.fps,
                                        run.low_fps,
                                        run.hitches_per_min,
                                        run.cpu_cores
                                            .map(|c| format!(" · CPU {c:.1} 核"))
                                            .unwrap_or_default()
                                    ))
                                    .size(13.0)
                                    .color(t::dim()),
                                );
                            }
                        }
                        for note in &result.notes {
                            ui.label(egui::RichText::new(note).size(13.0).color(t::dim()));
                        }
                    }
                    for note in &store.skipped {
                        ui.label(
                            egui::RichText::new(format!("没测：{note}"))
                                .size(13.0)
                                .color(t::dim()),
                        );
                    }
                }
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("额外 JVM 参数")
                        .size(13.0)
                        .color(t::sub()),
                );
                text_field(
                    ui,
                    &mut self.prefs.jvm_args,
                    "和自动参数冲突时以这里为准",
                    ui.available_width(),
                );
            });
    }

    /// 回收器那一行：测过没有、结论是什么、能不能再测。
    pub(crate) fn bench_status(
        &self,
        java: Option<&java::JavaRuntime>,
    ) -> (String, egui::Color32, Option<&'static str>) {
        let testable = !bench::candidates(&self.env.javas).0.is_empty();
        let Some(store) = &self.env.bench else {
            return if testable {
                (
                    "回收器按经验选；测一下这台电脑更准，约 1–2 分钟".into(),
                    t::dim(),
                    Some("测一下"),
                )
            } else {
                ("没有 Java 21+，回收器只能用 G1".into(), t::dim(), None)
            };
        };
        if !store.fresh() {
            return (
                "上次测试后电脑的核数或内存变了，结论不再使用".into(),
                t::warn(),
                testable.then_some("重新测试"),
            );
        }
        let age = bench::age(store.when);
        match java {
            Some(java) if java.major < 21 => (
                format!("这个版本用 Java {}，只用 G1，不需要测试", java.major),
                t::dim(),
                testable.then_some("重新测试"),
            ),
            Some(java) if java.is_graal() => (
                "这个版本用 GraalVM，只用 G1，不需要测试".into(),
                t::dim(),
                testable.then_some("重新测试"),
            ),
            Some(java) => match store.find(java) {
                Some(result) => {
                    let tiers = result
                        .tiers
                        .iter()
                        .map(|tier| format!("{}档 {}", tier.label, tier.collector.label()))
                        .collect::<Vec<_>>()
                        .join("、");
                    (
                        format!("{age}测过这台电脑：Java {} {tiers}", result.major),
                        t::dim(),
                        Some("重新测试"),
                    )
                }
                None => (
                    format!("{age}的测试里没有 Java {}，回收器按经验选", java.major),
                    t::dim(),
                    Some("重新测试"),
                ),
            },
            None => (format!("{age}测过这台电脑"), t::dim(), Some("重新测试")),
        }
    }

    /// 设置不再是首页的一块卡片，也不是第五个一级页面；它只在这里出现。
    pub(crate) fn settings_overlay(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let game_dir = self
            .selected_row()
            .map(|row| {
                row.game_dir
                    .clone()
                    .unwrap_or_else(|| self.root.version_dir(&row.profile))
            })
            .unwrap_or_else(|| self.root.dir().to_path_buf());
        let mut close = false;
        let mut open_account = false;
        let mut open_dir: Option<PathBuf> = None;
        let mut refresh = false;
        let mut recheck_java = false;
        let mut download_java = false;
        let mut check_update = false;
        let mut open_update = false;
        let mut open_log = false;
        let veil = t::veil(1.0);
        let modal = egui::Modal::new(egui::Id::unique("settings_modal"))
            .backdrop_color(veil)
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(26, 22)),
            )
            .show(ctx, |ui| {
                ui.set_width(470.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("设置")
                                .size(24.0)
                                .family(display_family()),
                        );
                        ui.label(
                            egui::RichText::new("账户、内存、Java 与本地目录")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.with_layout(egui::Layout::top_down(egui::Align::RIGHT), |ui| {
                                ui.label(
                                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                        .size(13.0)
                                        .color(t::sub()),
                                );
                                if let Some(version) = self.update.latest.as_ref().map(|r| r.version.clone()) {
                                    if action_link(ui, &format!("更新到 {version}")).clicked() {
                                        open_update = true;
                                    }
                                } else if self.update.checking {
                                    ui.label(egui::RichText::new("正在检查…").size(12.0).color(t::dim()));
                                } else {
                                    let label = self.update.status.clone().unwrap_or_else(|| "检查更新".into());
                                    if action_link(ui, &label).on_hover_text("查 GitHub 上的最新版本").clicked() {
                                        check_update = true;
                                    }
                                }
                            });
                        });
                    });
                });
                ui.add_space(14.0);
                let height = ctx.content_rect().height();
                let body_max = (height - 190.0).max(280.0);
                egui::ScrollArea::vertical()
                    .max_height(body_max)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    if let Some(account) = &self.account {
                                        avatar_sized(ui, &account.name, 30.0);
                                        ui.add_space(8.0);
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(&account.name)
                                                    .size(13.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new("Microsoft 账户 · 已登录")
                                                    .size(13.0)
                                                    .color(t::ok()),
                                            );
                                        });
                                    } else {
                                        avatar_sized(ui, &self.prefs.username, 30.0);
                                        ui.add_space(8.0);
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(&self.prefs.username)
                                                    .size(13.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new("离线账户 · 单人 / 局域网")
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                        });
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let label = if self.account.is_some() {
                                                "管理账户"
                                            } else {
                                                "登录 Microsoft"
                                            };
                                            if motion::secondary_sized(
                                                ui, label, true, 112.0, 30.0, 10.5,
                                            )
                                            .clicked()
                                            {
                                                open_account = true;
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(12.0);

                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("玩家名").size(13.0).color(t::sub()),
                                    );
                                    text_field(ui, &mut self.prefs.username, "", 150.0);
                                });
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    status_dot(ui, self.env.java_ok);
                                    let label = self.env.java_label.clone().unwrap_or_else(|| {
                                        if self.env.java_ok {
                                            "Java 21+ 已检测".to_string()
                                        } else {
                                            "未检测到 Java 21+".to_string()
                                        }
                                    });
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(short_text(&label, 34))
                                                .size(13.0)
                                                .color(if self.env.java_ok {
                                                    t::sub()
                                                } else {
                                                    t::warn()
                                                }),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&label);
                                    if action_link(ui, "重新检测").clicked() {
                                        recheck_java = true;
                                    }
                                    if !self.env.java_ok
                                        && action_link(ui, "下载 Java 21")
                                            .on_hover_text("下载 Mojang 官方的 Java 运行时，放在数据目录里，不改系统")
                                            .clicked()
                                    {
                                        download_java = true;
                                    }
                                });
                            });
                        ui.add_space(12.0);

                        self.tuning_card(ui);
                        ui.add_space(12.0);

                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    if path_row(ui, "游戏目录", &game_dir) {
                                        open_dir = Some(game_dir.clone());
                                    }
                                });
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if path_row(ui, "数据目录", self.root.dir()) {
                                        open_dir = Some(self.root.dir().to_path_buf());
                                    }
                                });
                            });
                        ui.add_space(12.0);

                        self.source_card(ui);
                        ui.add_space(12.0);

                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_width(442.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("外观").size(13.0).color(t::sub()),
                                    );
                                    ui.add_space(8.0);
                                    for pref in t::Preference::ALL {
                                        if chip(ui, pref.label(), self.prefs.theme == pref).clicked()
                                        {
                                            self.prefs.theme = pref;
                                            pref.apply(ui.ctx());
                                            self.save_ui_config();
                                        }
                                    }
                                });
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if toggle_switch(ui, self.prefs.reduce_motion).clicked() {
                                        self.prefs.reduce_motion = !self.prefs.reduce_motion;
                                        motion::set_reduced(self.prefs.reduce_motion);
                                    }
                                    ui.label(egui::RichText::new("减少动效").size(13.0).color(
                                        if self.prefs.reduce_motion {
                                            t::ink()
                                        } else {
                                            t::sub()
                                        },
                                    ));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new("关闭过渡和持续动画")
                                                    .size(13.0)
                                                    .color(t::sub()),
                                            );
                                        },
                                    );
                                });
                            });
                        ui.add_space(12.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("诊断")
                                    .size(13.0)
                                    .family(bold_family())
                                    .color(t::sub()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if action_link(ui, "刷新版本列表").clicked() {
                                        refresh = true;
                                    }
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(match &self.play.last {
                                    Some(last) => format!(
                                        "最近一次启动：{}",
                                        short_text(&last.label, 24)
                                    ),
                                    None => format!("启动器记录 {} 条", self.log.len()),
                                })
                                .size(13.0)
                                .color(t::sub()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if action_link(ui, "查看日志").clicked() {
                                        open_log = true;
                                    }
                                },
                            );
                        });
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(
                                "快捷键：Ctrl+1–4 切换页面 · Ctrl+F 搜索 · Ctrl+L 日志 · Ctrl+, 设置 · Esc 关闭弹窗",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if motion::secondary_sized(ui, "打开数据目录", true, 118.0, 32.0, 11.0)
                        .clicked()
                    {
                        open_dir = Some(self.root.dir().to_path_buf());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::primary_sized(ui, "完成", true, 86.0, 32.0, 12.0) {
                            close = true;
                        }
                    });
                });
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if open_account {
            self.save_ui_config();
            self.show_settings = false;
            if self.account.is_some() {
                self.login.open = true;
            } else {
                self.start_login();
            }
        }
        if let Some(dir) = open_dir {
            open_folder(&dir);
        }
        if refresh {
            self.refresh();
        }
        if recheck_java {
            self.refresh_java();
        }
        if check_update {
            self.check_update(ctx, true);
        }
        if open_update {
            self.show_settings = false;
            self.update.open = true;
        }
        if download_java {
            self.show_settings = false;
            self.start_java_download(21, None, None);
        }
        if close || self.bench_confirm || open_log {
            self.save_ui_config();
            self.show_settings = false;
        }
        if open_log {
            self.open_log();
        }
    }
}

/// 两种下载源。镜像只是后备或加速，文件照样按官方给的校验值核对。
const SOURCES: [(&str, &str, &str); 2] = [
    ("official", "官方优先", "先连官方，连不上自动换镜像"),
    (
        "bmcl",
        "镜像优先",
        "游戏文件和加载器先走 BMCLAPI，国内通常更快；Mod 平台仍先连官方",
    ),
];

impl App {
    /// 下载源和镜像致谢。
    pub(crate) fn source_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(t::bg())
            .stroke(egui::Stroke::new(1.0, t::hairline()))
            .corner_radius(motion::R)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(442.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("下载源").size(13.0).color(t::sub()));
                    ui.add_space(8.0);
                    for (key, label, _) in SOURCES {
                        if chip(ui, label, self.prefs.source == key).clicked() {
                            self.prefs.source = key.to_string();
                            self.save_ui_config();
                        }
                    }
                });
                ui.add_space(4.0);
                let hint = SOURCES
                    .iter()
                    .find(|(key, _, _)| *key == self.prefs.source)
                    .map_or("", |(_, _, hint)| hint);
                ui.label(egui::RichText::new(hint).size(12.5).color(t::sub()));
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let dim = |text: &str| egui::RichText::new(text).size(12.5).color(t::dim());
                    let link = |text: &str| egui::RichText::new(text).size(12.5);
                    ui.label(dim("镜像由 "));
                    if ui
                        .link(link("BMCLAPI"))
                        .on_hover_text("bangbang93 维护的 Minecraft 下载镜像 · 点开看说明和赞助")
                        .clicked()
                    {
                        open_url("https://bmclapidoc.bangbang93.com/");
                    }
                    ui.label(dim(" 和 "));
                    if ui
                        .link(link("MCIM"))
                        .on_hover_text(
                            "mcmod-info-mirror 维护的 Modrinth / CurseForge 接口镜像 · 只在官方连不上时用",
                        )
                        .clicked()
                    {
                        open_url("https://github.com/mcmod-info-mirror");
                    }
                    ui.label(dim(" 免费提供，感谢维护者"));
                });
            });
    }
}

/// 内存：「自动」加 1 GB 步进。自动时显示算出来的值，按加减就转成手动。
pub(crate) fn memory_stepper(ui: &mut egui::Ui, memory: &mut Option<u32>, auto_mb: u32) {
    if chip(ui, "自动", memory.is_none()).clicked() {
        *memory = None;
    }
    ui.add_space(4.0);
    let (down, up) = match *memory {
        Some(mb) => {
            let gb = (mb as f32 / 1024.0).round() as u32;
            (gb.saturating_sub(1), gb + 1)
        }
        None => (auto_mb.div_ceil(1024).saturating_sub(1), auto_mb / 1024 + 1),
    };
    let button = |ui: &mut egui::Ui, text: &str, enabled: bool| {
        ui.add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new(text).size(13.0).color(t::sub()))
                .fill(t::bg())
                .stroke(egui::Stroke::new(1.0, t::hairline()))
                .corner_radius(motion::R)
                .min_size(egui::vec2(28.0, 28.0)),
        )
        .clicked()
    };
    if button(ui, "−", down >= 1) {
        *memory = Some(down * 1024);
    }
    let (text, color) = match *memory {
        Some(mb) => (tuning::gb(mb), t::ink()),
        None => (tuning::gb(auto_mb), t::sub()),
    };
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(1.0, t::hairline()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.add_sized(
                [52.0, 18.0],
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(13.0)
                        .color(color)
                        .family(bold_family()),
                ),
            );
        });
    if button(ui, "+", up <= 16) {
        *memory = Some(up * 1024);
    }
}
