//! 页面导航和侧边栏。五个一级页面：启动台、版本、内容、存档、截图。

use crate::prelude::*;

pub(crate) mod content;
pub(crate) mod gallery;
pub(crate) mod home;
pub(crate) mod progress;
pub(crate) mod shots;
pub(crate) mod versions;
pub(crate) mod worlds;

impl App {
    /// 当前侧边栏应该高亮哪一项。安装页属于「版本」的子流程。
    pub(crate) fn nav_active(&self) -> Nav {
        if matches!(self.busy, Some(Task::Install)) {
            Nav::Versions
        } else if self.show_gallery {
            Nav::Shots
        } else if self.show_mods {
            Nav::Content
        } else if self.show_saves {
            Nav::Saves
        } else if self.show_manage || self.show_install {
            Nav::Versions
        } else {
            Nav::Home
        }
    }

    pub(crate) fn nav_go(&mut self, nav: Nav) {
        if self.page_locked() {
            return;
        }
        if self.nav_active() == nav {
            // 再点一次当前入口：只做“回到这个入口的默认视图”，不重置数据。
            match nav {
                Nav::Versions if self.show_install => {
                    self.show_install = false;
                    self.show_manage = true;
                    self.install.repair_target = None;
                }
                Nav::Content if !self.content.show_installed => self.open_content(true),
                // 再点一次「截图」回到总览。
                Nav::Shots => {
                    self.gallery.tab = gallery::GalleryTab::Overview;
                    self.gallery.filter = gallery::Filter::default();
                    self.gallery.query.clear();
                }
                _ => {}
            }
            return;
        }
        if nav != Nav::Shots {
            self.show_gallery = false;
        }
        match nav {
            Nav::Home => {
                self.show_mods = false;
                self.show_saves = false;
                self.show_manage = false;
                self.show_install = false;
                self.install.repair_target = None;
                self.error = None;
                self.notice = None;
                // 首页只是切视图：版本列表已在安装/删除/内容变更后刷新过。
                // 不再在这里同步 scan 每个版本的 health，否则点击会卡一下。
                self.ensure_visible_selection();
            }
            Nav::Versions => {
                if self.visible_count() == 0 {
                    self.open_install();
                } else {
                    self.show_mods = false;
                    self.show_saves = false;
                    self.show_install = false;
                    self.show_manage = true;
                    self.install.repair_target = None;
                    self.delete_confirm = None;
                    self.error = None;
                    self.ensure_visible_selection();
                }
            }
            Nav::Content => self.open_content(true),
            Nav::Saves => self.open_saves_page(),
            Nav::Shots => self.open_gallery(None),
        }
    }

    /// 内容只有一页：发现和已安装是同一页的两种模式。
    pub(crate) fn open_content(&mut self, installed: bool) {
        if self.page_locked() {
            return;
        }
        let Some(row) = self
            .selected_row()
            .or_else(|| self.visible_rows().next().cloned())
        else {
            self.open_install();
            return;
        };
        self.selected = Some(row.id.clone());
        self.show_saves = false;
        self.show_manage = false;
        self.show_install = false;
        self.error = None;
        self.notice = None;
        let same_target = self
            .content
            .row
            .as_ref()
            .is_some_and(|current| current.id == row.id);
        if !self.show_mods || !same_target {
            self.enter_mods(Some(row));
        }
        self.clear_mods_selection();
        self.content.show_installed = installed;
        self.content.filter.clear();
        self.content.delete_confirm = None;
        self.content.error = None;
        self.content.notice = None;
        if installed {
            if self.content.game_dir.is_some() {
                self.start_mods_scan();
            }
        } else {
            self.content.searching = false;
            self.content.shelf = None;
            self.content.query.clear();
            if self.content.browse.is_empty() && !self.content.browse_loading {
                self.start_browse();
            }
        }
    }

    pub(crate) fn open_saves_page(&mut self) {
        if self.page_locked() {
            return;
        }
        let Some(row) = self
            .selected_row()
            .or_else(|| self.visible_rows().next().cloned())
        else {
            self.open_install();
            return;
        };
        self.selected = Some(row.id.clone());
        self.open_saves(row);
    }

    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui) {
        let wide = ui.ctx().content_rect().width() >= 1080.0;
        let width = if wide { 190.0 } else { 64.0 };
        let active = self.nav_active();
        let current = self.current_account().cloned();
        let face = self.face(ui.ctx(), current.as_ref().and_then(|a| a.skin.as_deref()));
        let mut account_rect = None;
        let busy = self.busy.is_some();
        let login_busy = self.login.busy;
        let enabled = !self.page_locked();
        let mut go: Option<Nav> = None;
        let mut account_clicked = false;
        let mut update_clicked = false;
        let update_version = self.update.badge(&self.prefs.update_skip).map(String::from);
        let mut settings_clicked = false;

        egui::Panel::left("shell_nav")
            .resizable(false)
            .exact_size(width)
            .frame(egui::Frame::new().fill(t::bg()).inner_margin(egui::Margin {
                left: if wide { 12 } else { 10 },
                right: if wide { 12 } else { 10 },
                top: 20,
                bottom: 14,
            }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                if wide {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Cairn")
                                .size(21.0)
                                .family(light_family())
                                .color(t::ink()),
                        );
                    });
                } else {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("C")
                                .size(24.0)
                                .family(light_family())
                                .color(t::ink()),
                        );
                    });
                }
                ui.add_space(20.0);

                for nav in Nav::ALL {
                    if sidebar_item(ui, nav, active == nav, wide, enabled).clicked() {
                        go = Some(nav);
                    }
                }

                // 底部用显式留白定位：嵌套 bottom_up 在侧边栏里会缩成内容高度。
                let playing = matches!(self.busy, Some(Task::Launch));
                let locked = (busy && !playing) || login_busy;
                let footer = if update_version.is_some() && !locked {
                    38.0
                } else {
                    0.0
                } + if locked {
                    40.0
                } else if playing {
                    112.0
                } else {
                    78.0
                };
                ui.add_space((ui.available_height() - footer).max(10.0));
                if playing {
                    ui.horizontal(|ui| {
                        ui.add_space(if wide { 14.0 } else { 18.0 });
                        status_dot(ui, true);
                        if wide {
                            ui.label(egui::RichText::new("游戏运行中").size(13.0).color(t::sub()));
                        }
                    })
                    .response
                    .on_hover_text("游戏运行中：可以查看各页，正在玩的版本退出后才能修改");
                    ui.add_space(8.0);
                }
                if locked {
                    ui.horizontal(|ui| {
                        if wide {
                            ui.add_space(12.0);
                        }
                        busy_spinner(ui);
                        if wide {
                            ui.label(
                                egui::RichText::new(if login_busy {
                                    "正在登录…"
                                } else {
                                    "任务进行中"
                                })
                                .size(13.0)
                                .color(t::sub()),
                            );
                        }
                    });
                } else {
                    if let Some(version) = &update_version {
                        if update_pill(ui, version, wide) {
                            update_clicked = true;
                        }
                        ui.add_space(8.0);
                    }
                    let response = sidebar_account(ui, current.as_ref(), face.as_ref(), wide);
                    account_rect = Some(response.rect);
                    if response.clicked() {
                        account_clicked = true;
                    }
                    if sidebar_settings(ui, wide, true).clicked() {
                        settings_clicked = true;
                    }
                }
            });

        if let Some(nav) = go {
            self.nav_go(nav);
        }
        if account_rect.is_some() {
            self.login.anchor = account_rect;
        }
        if account_clicked {
            self.login.open = !self.login.open;
        }
        if update_clicked {
            self.update.open = true;
        }
        if settings_clicked {
            self.login.open = false;
            self.show_settings = true;
        }
    }
}

/// 侧边栏的一级入口：宽窗口带文字，窄窗口只有图标。
pub(crate) fn nav_row(
    ui: &mut egui::Ui,
    height: f32,
    active: bool,
    enabled: bool,
) -> (egui::Rect, egui::Response, egui::Color32) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let hover = motion::hover_amount(ui, response.id.with("hover"), enabled && response.hovered());
    if active {
        ui.painter().rect_filled(rect, motion::R, t::wash());
    } else if hover > 0.0 {
        ui.painter()
            .rect_filled(rect, motion::R, t::tint(t::wash(), hover));
    }
    let color = if !enabled {
        t::dim()
    } else if active {
        t::ink()
    } else {
        motion::mix(t::sub(), t::ink(), hover)
    };
    (rect, response, color)
}

/// 侧边栏的一级入口：宽窗口带文字，窄窗口只有图标。
pub(crate) fn sidebar_item(
    ui: &mut egui::Ui,
    nav: Nav,
    active: bool,
    wide: bool,
    enabled: bool,
) -> egui::Response {
    let (rect, response, color) = nav_row(ui, 36.0, active, enabled);
    let icon = if wide {
        egui::pos2(rect.left() + 21.0, rect.center().y)
    } else {
        rect.center()
    };
    nav_glyph(ui.painter(), icon, nav, color);
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            nav.label(),
            egui::FontId::proportional(14.0),
            color,
        );
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response.on_hover_text("任务进行中，完成后再切换")
    }
    .on_hover_text(if wide { nav.hint() } else { nav.label() })
}

pub(crate) fn nav_glyph(painter: &egui::Painter, c: egui::Pos2, nav: Nav, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.4, color);
    let p = |x: f32, y: f32| c + egui::vec2(x, y);
    match nav {
        Nav::Home => {
            painter.add(egui::Shape::closed_line(
                vec![p(-4.5, -7.0), p(7.0, 0.0), p(-4.5, 7.0)],
                stroke,
            ));
        }
        Nav::Versions => {
            for y in [-4.0, 0.0, 4.0] {
                painter.add(egui::Shape::closed_line(
                    vec![p(0.0, y - 3.5), p(7.5, y), p(0.0, y + 3.5), p(-7.5, y)],
                    stroke,
                ));
            }
        }
        Nav::Content => {
            for (x, y) in [(-7.0, -7.0), (1.0, -7.0), (-7.0, 1.0), (1.0, 1.0)] {
                painter.rect_stroke(
                    egui::Rect::from_min_size(p(x, y), egui::vec2(6.0, 6.0)),
                    1.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        Nav::Shots => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(15.0, 12.0)),
                1.5,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(
                vec![
                    p(-5.5, 4.0),
                    p(-2.0, 0.0),
                    p(0.8, 2.6),
                    p(2.6, 1.0),
                    p(5.5, 4.0),
                ],
                stroke,
            ));
            painter.circle_filled(p(3.6, -2.4), 1.1, color);
        }
        Nav::Saves => {
            painter.circle_stroke(c, 7.5, stroke);
            painter.line_segment([p(-7.5, 0.0), p(7.5, 0.0)], stroke);
            painter.add(egui::Shape::ellipse_stroke(c, egui::vec2(3.2, 7.5), stroke));
        }
    }
}

pub(crate) fn sidebar_account(
    ui: &mut egui::Ui,
    account: Option<&nailong_core::accounts::Account>,
    face: Option<&egui::TextureHandle>,
    wide: bool,
) -> egui::Response {
    let (rect, response, color) = nav_row(ui, 36.0, false, true);
    let name = account.map(|a| a.name.as_str()).unwrap_or("添加账户");
    let center = if wide {
        egui::pos2(rect.left() + 21.0, rect.center().y)
    } else {
        rect.center()
    };
    let icon = egui::Rect::from_center_size(center, egui::vec2(22.0, 22.0));
    draw_avatar(ui.painter(), icon, name, face);
    if account.is_some_and(|a| a.stale) {
        ui.painter().circle(
            icon.right_top(),
            3.5,
            t::warn(),
            egui::Stroke::new(1.5, t::bg()),
        );
    }
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            short_text(name, 14),
            egui::FontId::proportional(14.0),
            color,
        );
    }
    let hint = match account {
        Some(a) if a.stale => format!("{} · 需要重新登录", a.name),
        Some(a) => format!("{} · {}", a.name, a.kind_label()),
        None => "添加账户".into(),
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
}

/// 侧边栏底部的工具入口：设置不是一级页面，做成安静的滑杆图标。
pub(crate) fn sidebar_settings(ui: &mut egui::Ui, wide: bool, enabled: bool) -> egui::Response {
    let (rect, response, color) = nav_row(ui, 36.0, false, enabled);
    let center = if wide {
        egui::pos2(rect.left() + 21.0, rect.center().y)
    } else {
        rect.center()
    };
    let stroke = egui::Stroke::new(1.4, color);
    for (y, x) in [(-4.0, -2.5), (0.0, 3.0), (4.0, -0.5)] {
        ui.painter().line_segment(
            [center + egui::vec2(-7.5, y), center + egui::vec2(7.5, y)],
            stroke,
        );
        ui.painter()
            .circle(center + egui::vec2(x, y), 2.0, t::bg(), stroke);
    }
    if wide {
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "设置",
            egui::FontId::proportional(14.0),
            color,
        );
    }
    if enabled {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("设置：外观、内存、Java、目录与日志（Ctrl+,）")
    } else {
        response.on_hover_text("任务进行中")
    }
}
