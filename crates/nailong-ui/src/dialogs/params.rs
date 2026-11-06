//! 启动参数：启动台「开始游戏」下面的一排参数，和按版本修改它们的窗口。
//!
//! 启动前就能看到这次会用哪个 Java、多大内存、哪个回收器、多大窗口、几个额外参数，
//! 点任何一个都能改；改的只是这个版本，没改的跟随全局设置。窗口里有完整的命令行预览，
//! 和真正启动时由同一段代码生成，可以复制。

use crate::prelude::*;

/// 一个参数胶囊：上面的标签小、下面的值清楚；单独设置过的有强调色边框。
fn pill(ui: &mut egui::Ui, label: &str, value: &str, custom: bool, warn: bool) -> egui::Response {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(12.0), t::dim()),
    );
    job.append(
        value,
        6.0,
        egui::TextFormat::simple(
            egui::FontId::proportional(13.0),
            if warn { t::warn() } else { t::ink() },
        ),
    );
    let stroke = if custom {
        t::tint(t::accent(), 0.7)
    } else {
        t::hairline()
    };
    let response = ui.add(
        egui::Button::new(job)
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(motion::R)
            .min_size(egui::vec2(0.0, 26.0)),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

impl App {
    pub(crate) fn open_launch_edit(&mut self, row: &Row) {
        let draft = self.version_cfg(&row.id);
        let [width, height] = draft.window();
        self.launch_edit = Some(LaunchEdit {
            row_id: row.id.clone(),
            draft,
            width: width.to_string(),
            height: height.to_string(),
            preview_key: String::new(),
            preview_rx: None,
            preview: None,
            copied_at: None,
        });
    }

    /// 启动台上「开始游戏」下面的一排：这次会怎么启动。点了打开启动参数窗口。
    pub(crate) fn launch_pills(&mut self, ui: &mut egui::Ui, row: &Row) {
        let cfg = self.version_cfg(&row.id);
        let planned = self.plan_for(row, &cfg);
        let mut open = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let (java, java_warn) = match (&planned, &cfg.java) {
                (Some((_, java)), _) => (format!("Java {}", java.major), false),
                (None, Some(_)) => ("指定的 Java 不可用".to_string(), true),
                (None, None) => ("没有合适的 Java".to_string(), true),
            };
            let java_tip = match &planned {
                Some((_, java)) => format!("{}\n{}", java.label(), java.path.display()),
                None => "在启动参数里选一个 Java".to_string(),
            };
            if pill(ui, "Java", &java, cfg.java.is_some(), java_warn)
                .on_hover_text(java_tip)
                .clicked()
            {
                open = true;
            }
            if let Some((plan, _)) = &planned {
                let memory = if plan.auto_memory {
                    format!("{} 自动", tuning::gb(plan.heap_mb))
                } else {
                    tuning::gb(plan.heap_mb)
                };
                if pill(ui, "内存", &memory, cfg.memory.is_some(), false)
                    .on_hover_text(&plan.basis)
                    .clicked()
                {
                    open = true;
                }
                let gc = cfg.gc.unwrap_or(self.prefs.gc);
                let collector = if gc == tuning::GcChoice::Auto {
                    format!("{} 自动", plan.collector.label())
                } else {
                    plan.collector.label().to_string()
                };
                if pill(ui, "GC", &collector, cfg.gc.is_some(), false)
                    .on_hover_text("垃圾回收器")
                    .clicked()
                {
                    open = true;
                }
            }
            let [width, height] = cfg.window();
            if pill(
                ui,
                "窗口",
                &format!("{width}×{height}"),
                cfg.window.is_some(),
                false,
            )
            .clicked()
            {
                open = true;
            }
            let extra = tuning::split_args(cfg.jvm_args.as_deref().unwrap_or(&self.prefs.jvm_args));
            if !extra.is_empty() || cfg.jvm_args.is_some() {
                let value = if extra.is_empty() {
                    "无".to_string()
                } else {
                    format!("{} 个", extra.len())
                };
                if pill(ui, "JVM 参数", &value, cfg.jvm_args.is_some(), false)
                    .on_hover_text(extra.join(" "))
                    .clicked()
                {
                    open = true;
                }
            }
            if pill(ui, "", "调整", false, false)
                .on_hover_text("改这个版本的 Java、内存、回收器和窗口，看完整命令行")
                .clicked()
            {
                open = true;
            }
        });
        if open {
            self.open_launch_edit(row);
        }
    }

    /// 启动参数窗口。
    pub(crate) fn launch_overlay(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.launch_edit.take() else {
            return;
        };
        let Some(row) = self.rows.iter().find(|r| r.id == edit.row_id).cloned() else {
            return;
        };
        let required = launch::required_java_for(&self.root, &row.profile).unwrap_or(8);
        let planned = self.plan_for(&row, &edit.draft);
        let global = VersionLaunch::default();
        let global_plan = self.plan_for(&row, &global).map(|(plan, _)| plan);
        self.poll_preview(ctx, &row, &mut edit, planned.as_ref().map(|(_, java)| java));

        let mut javas: Vec<java::JavaRuntime> = self.env.javas.clone();
        for extra in &self.env.extra_javas {
            if !javas.iter().any(|j| j.path == extra.path) {
                javas.push(extra.clone());
            }
        }
        let auto_java = java::select(required, &self.env.javas).ok();
        let now = ctx.input(|i| i.time);
        let mut save = false;
        let mut close = false;
        let mut pick_java = false;
        let mut download_java = false;
        let screen = ctx.content_rect();
        let width = (screen.width() - 120.0).clamp(520.0, 680.0);
        let modal = egui::Modal::new(egui::Id::unique("launch_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(motion::R)
                    .inner_margin(egui::Margin::symmetric(24, 20)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("启动参数")
                                .size(24.0)
                                .family(display_family()),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · 只改这个版本，没改的跟随全局（设置 › 性能）",
                                short_text(&row.label, 24)
                            ))
                            .size(13.0)
                            .color(t::sub()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("×").size(18.0)).frame(false),
                            )
                            .on_hover_text("不保存（Esc）")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.add_space(12.0);
                let body_h = (screen.height() - 200.0).clamp(240.0, 560.0);
                egui::ScrollArea::vertical()
                    .max_height(body_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(width - 8.0);
                        // Java：自动，或者本机的任意一个，或者手动选。
                        section(ui, "Java");
                        let auto_label = match &auto_java {
                            Some(java) => format!("自动 · {}", java.label()),
                            None => format!("自动 · 没有找到 Java {required}+"),
                        };
                        if java_row(
                            ui,
                            edit.draft.java.is_none(),
                            &auto_label,
                            &format!("这个版本需要 Java {required} 或更新，自动选够用里最低的"),
                            None,
                        ) {
                            edit.draft.java = None;
                        }
                        for java in &javas {
                            let selected = edit.draft.java.as_deref() == Some(java.path.as_path());
                            let warn = (java.major < required)
                                .then(|| format!("版本太低，这个版本需要 Java {required}"));
                            if java_row(
                                ui,
                                selected,
                                &java.label(),
                                &java.path.display().to_string(),
                                warn.as_deref(),
                            ) {
                                edit.draft.java = Some(java.path.clone());
                            }
                        }
                        if let Some(path) = &edit.draft.java
                            && !javas.iter().any(|j| &j.path == path)
                        {
                            java_row(
                                ui,
                                true,
                                "找不到这个 Java",
                                &path.display().to_string(),
                                Some("文件不在了，换一个"),
                            );
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if action_link(ui, "选择别的 java 程序…").clicked() {
                                pick_java = true;
                            }
                            if action_link(ui, &format!("下载官方 Java {required}"))
                                .on_hover_text("Mojang 官方的运行时，放在数据目录里，不改系统；下好后「自动」会用它")
                                .clicked()
                            {
                                download_java = true;
                            }
                        });

                        // 内存。
                        ui.add_space(12.0);
                        section(ui, "内存");
                        let follow_memory = global_plan
                            .as_ref()
                            .map(|plan| {
                                if self.prefs.memory.is_none() {
                                    format!("跟随全局 · 自动 {}", tuning::gb(plan.heap_mb))
                                } else {
                                    format!("跟随全局 · {}", tuning::gb(plan.heap_mb))
                                }
                            })
                            .unwrap_or_else(|| "跟随全局".into());
                        ui.horizontal(|ui| {
                            if chip(ui, &follow_memory, edit.draft.memory.is_none()).clicked() {
                                edit.draft.memory = None;
                            }
                            if edit.draft.memory.is_none() {
                                if chip(ui, "单独设置", false).clicked() {
                                    edit.draft.memory = Some(MemoryPick::Auto);
                                }
                            } else {
                                let auto_mb =
                                    planned.as_ref().map_or(4096, |(plan, _)| plan.heap_mb);
                                let mut memory = edit.draft.memory.and_then(MemoryPick::memory_mb);
                                memory_stepper(ui, &mut memory, auto_mb);
                                edit.draft.memory =
                                    Some(memory.map_or(MemoryPick::Auto, MemoryPick::Mb));
                            }
                        });

                        // 回收器。
                        ui.add_space(12.0);
                        section(ui, "回收器");
                        ui.horizontal(|ui| {
                            let follow = format!("跟随全局 · {}", self.prefs.gc.label());
                            if chip(ui, &follow, edit.draft.gc.is_none()).clicked() {
                                edit.draft.gc = None;
                            }
                            for gc in tuning::GcChoice::ALL {
                                if chip(ui, gc.label(), edit.draft.gc == Some(gc)).clicked() {
                                    edit.draft.gc = Some(gc);
                                }
                            }
                        });

                        // 窗口大小。
                        ui.add_space(12.0);
                        section(ui, "游戏窗口");
                        ui.horizontal_wrapped(|ui| {
                            let [dw, dh] = launchcfg::DEFAULT_WINDOW;
                            if chip(ui, &format!("默认 {dw}×{dh}"), edit.draft.window.is_none())
                                .clicked()
                            {
                                edit.draft.window = None;
                                edit.width = dw.to_string();
                                edit.height = dh.to_string();
                            }
                            for size in [[854, 480], [1600, 900], [1920, 1080]] {
                                if chip(
                                    ui,
                                    &format!("{}×{}", size[0], size[1]),
                                    edit.draft.window == Some(size),
                                )
                                .clicked()
                                {
                                    edit.draft.window = Some(size);
                                    edit.width = size[0].to_string();
                                    edit.height = size[1].to_string();
                                }
                            }
                            ui.add_space(6.0);
                            let w = text_field(ui, &mut edit.width, "宽", 54.0);
                            ui.label(egui::RichText::new("×").size(13.0).color(t::dim()));
                            let h = text_field(ui, &mut edit.height, "高", 54.0);
                            if (w.changed() || h.changed())
                                && let (Ok(w), Ok(h)) = (
                                    edit.width.trim().parse::<u32>(),
                                    edit.height.trim().parse::<u32>(),
                                )
                                && (320..=7680).contains(&w)
                                && (240..=4320).contains(&h)
                            {
                                edit.draft.window = (Some([w, h])
                                    != Some(launchcfg::DEFAULT_WINDOW))
                                .then_some([w, h]);
                            }
                        });

                        // 额外 JVM 参数。
                        ui.add_space(12.0);
                        section(ui, "额外 JVM 参数");
                        ui.horizontal(|ui| {
                            let follow = if self.prefs.jvm_args.trim().is_empty() {
                                "跟随全局 · 无".to_string()
                            } else {
                                "跟随全局".to_string()
                            };
                            if chip(ui, &follow, edit.draft.jvm_args.is_none()).clicked() {
                                edit.draft.jvm_args = None;
                            }
                            if chip(ui, "单独设置", edit.draft.jvm_args.is_some()).clicked()
                                && edit.draft.jvm_args.is_none()
                            {
                                edit.draft.jvm_args = Some(self.prefs.jvm_args.clone());
                            }
                        });
                        if let Some(args) = &mut edit.draft.jvm_args {
                            ui.add_space(4.0);
                            text_field(
                                ui,
                                args,
                                "和自动参数冲突时以这里为准",
                                ui.available_width(),
                            );
                        } else if !self.prefs.jvm_args.trim().is_empty() {
                            ui.label(
                                egui::RichText::new(&self.prefs.jvm_args)
                                    .size(12.5)
                                    .monospace()
                                    .color(t::dim()),
                            );
                        }

                        // 结果和命令行。
                        ui.add_space(14.0);
                        match &planned {
                            Some((plan, java)) => {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · Java {} · {}",
                                        plan.headline(),
                                        java.major,
                                        plan.basis
                                    ))
                                    .size(13.0)
                                    .color(t::sub()),
                                );
                                for warning in &plan.warnings {
                                    ui.label(
                                        egui::RichText::new(warning).size(13.0).color(t::warn()),
                                    );
                                }
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("选的 Java 不可用，换一个才能启动")
                                        .size(13.0)
                                        .color(t::warn()),
                                );
                            }
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            section(ui, "命令行");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let copied = edit.copied_at.is_some_and(|at| now - at < 1.5);
                                    let ready = matches!(edit.preview, Some(Ok(_)));
                                    let label = if copied {
                                        "已复制"
                                    } else {
                                        "复制命令行"
                                    };
                                    if ui
                                        .add_enabled_ui(ready, |ui| action_link(ui, label))
                                        .inner
                                        .on_hover_text(
                                            "完整命令，粘贴到终端就能启动；访问令牌已打码",
                                        )
                                        .clicked()
                                        && let Some(Ok((_, command))) = &edit.preview
                                    {
                                        ui.ctx().copy_text(command.clone());
                                        edit.copied_at = Some(now);
                                        ui.ctx().request_repaint_after(Duration::from_millis(1600));
                                    }
                                },
                            );
                        });
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                match &edit.preview {
                                    Some(Ok((lines, _))) => {
                                        egui::ScrollArea::vertical()
                                            .id_salt("launch_command")
                                            .max_height(170.0)
                                            .auto_shrink([false, true])
                                            .show(ui, |ui| {
                                                ui.spacing_mut().item_spacing.y = 2.0;
                                                for line in lines {
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(line)
                                                                .monospace()
                                                                .size(12.0)
                                                                .color(t::sub()),
                                                        )
                                                        .wrap(),
                                                    );
                                                }
                                            });
                                    }
                                    Some(Err(e)) => {
                                        ui.label(
                                            egui::RichText::new(e).size(13.0).color(t::warn()),
                                        );
                                    }
                                    None => {
                                        ui.horizontal(|ui| {
                                            busy_spinner(ui);
                                            ui.label(
                                                egui::RichText::new("正在生成…")
                                                    .size(13.0)
                                                    .color(t::dim()),
                                            );
                                        });
                                    }
                                }
                            });
                    });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let custom = !edit.draft.is_default();
                    if ui
                        .add_enabled_ui(custom, |ui| action_link(ui, "全部跟随全局"))
                        .inner
                        .clicked()
                    {
                        edit.draft = VersionLaunch::default();
                        let [w, h] = launchcfg::DEFAULT_WINDOW;
                        edit.width = w.to_string();
                        edit.height = h.to_string();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if motion::primary_sized(ui, "保存", true, 86.0, 32.0, 12.0) {
                            save = true;
                        }
                        ui.add_space(6.0);
                        if motion::secondary_sized(ui, "取消", true, 72.0, 32.0, 12.0).clicked() {
                            close = true;
                        }
                    });
                });
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if download_java {
            let component = launch::java_requirement(&self.root, &row.profile)
                .ok()
                .and_then(|(_, component)| component);
            self.start_java_download(required, component, None);
            return;
        }
        if pick_java
            && let Some(path) = rfd::FileDialog::new()
                .set_title("选择 java 程序（bin 目录里的 java / javaw）")
                .pick_file()
        {
            match java::probe(&path) {
                Ok(runtime) => {
                    if !self.env.extra_javas.iter().any(|j| j.path == runtime.path) {
                        self.env.extra_javas.push(runtime.clone());
                    }
                    edit.draft.java = Some(runtime.path);
                }
                Err(_) => {
                    edit.preview = Some(Err(format!("这不是能用的 Java：{}", path.display())));
                }
            }
        }
        if save {
            match launchcfg::save(&self.root, &row.id, &edit.draft) {
                Ok(()) => {
                    if edit.draft.is_default() {
                        self.play.versions.remove(&row.id);
                    } else {
                        self.play
                            .versions
                            .insert(row.id.clone(), edit.draft.clone());
                    }
                    self.notice = Some(format!(
                        "已保存「{}」的启动参数",
                        short_text(&row.label, 20)
                    ));
                    self.error = None;
                }
                Err(e) => self.error = Some(format!("启动参数没保存上：{e}")),
            }
            return;
        }
        if !close {
            self.launch_edit = Some(edit);
        }
    }

    /// 草稿变了就在后台重新生成命令行预览；同一时间只跑一个。
    fn poll_preview(
        &mut self,
        ctx: &egui::Context,
        row: &Row,
        edit: &mut LaunchEdit,
        java: Option<&java::JavaRuntime>,
    ) {
        if let Some(rx) = &edit.preview_rx {
            match rx.try_recv() {
                Ok(result) => {
                    edit.preview = Some(result);
                    edit.preview_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => edit.preview_rx = None,
            }
        }
        let Some(java) = java else {
            edit.preview = Some(Err("选的 Java 不可用".into()));
            edit.preview_key.clear();
            return;
        };
        let settings = self.tuning_settings(&edit.draft);
        let [width, height] = edit.draft.window();
        let opts = LaunchOptions {
            username: self.prefs.username.clone(),
            memory_mb: settings.memory_mb,
            gc: settings.gc,
            jvm_args: settings.extra,
            game_dir: row.game_dir.clone(),
            java: Some(java.path.clone()),
            width,
            height,
            ..Default::default()
        };
        let key = format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{}",
            opts.memory_mb, opts.gc, opts.jvm_args, opts.java, width, height
        );
        if key == edit.preview_key {
            return;
        }
        edit.preview_key = key;
        let (tx, rx) = mpsc::channel();
        edit.preview_rx = Some(rx);
        let root = self.root.clone();
        let profile = row.profile.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = launch::preview(&root, &profile, &opts)
                .map(|prepared| (prepared.display_lines(), prepared.command_line()))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(13.0)
            .family(bold_family())
            .color(t::sub()),
    );
    ui.add_space(4.0);
}

/// Java 列表的一行：单选点、名字、路径；版本不够时给一句提醒。返回是否被点了。
fn java_row(
    ui: &mut egui::Ui,
    selected: bool,
    title: &str,
    detail: &str,
    warn: Option<&str>,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 40.0), egui::Sense::click());
    list_row_fill(ui, rect, selected, response.hovered());
    let painter = ui.painter();
    let dot = egui::pos2(rect.left() + 16.0, rect.center().y);
    painter.circle_stroke(
        dot,
        6.0,
        egui::Stroke::new(1.3, if selected { t::accent() } else { t::dim() }),
    );
    if selected {
        painter.circle_filled(dot, 3.2, t::accent());
    }
    painter.text(
        egui::pos2(rect.left() + 32.0, rect.top() + 12.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        if warn.is_some() { t::warn() } else { t::ink() },
    );
    let detail = match warn {
        Some(warn) => format!("{warn} · {detail}"),
        None => detail.to_string(),
    };
    let font = egui::FontId::proportional(12.0);
    let shown = elide_middle(ui, &detail, &font, width - 44.0);
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, rect.top() + 29.0),
        egui::Align2::LEFT_CENTER,
        shown,
        font,
        t::dim(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}
