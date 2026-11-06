//! 账户弹窗和微软登录。

use crate::prelude::*;

impl App {
    pub(crate) fn account_overlay(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        let kind = if self.login.error.is_some() {
            3
        } else if self.login.url.is_some() {
            1
        } else if self.login.open && self.account.is_some() {
            2
        } else {
            0
        };
        if kind != self.modal_key {
            self.modal_key = kind;
            self.modal_started = now;
        }
        let raw = if self.prefs.reduce_motion {
            1.0
        } else {
            (((now - self.modal_started) / 0.15) as f32).clamp(0.0, 1.0)
        };
        let open_e = motion::ease_out_cubic(raw);
        if kind != 0 && raw < 1.0 {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let backdrop = (178.0 * open_e) as u8;
        let veil = t::veil(backdrop as f32 / 178.0);
        let frame_fill = egui::Color32::from_rgba_unmultiplied(
            t::surface().r(),
            t::surface().g(),
            t::surface().b(),
            (255.0 * open_e) as u8,
        );
        let frame_stroke = egui::Color32::from_rgba_unmultiplied(
            t::hairline().r(),
            t::hairline().g(),
            t::hairline().b(),
            (255.0 * open_e) as u8,
        );
        if let Some(url) = self.login.url.clone() {
            let mut open = false;
            let mut cancel_login = false;
            let modal = egui::Modal::new(egui::Id::unique("login_modal"))
                .backdrop_color(veil)
                .frame(
                    egui::Frame::new()
                        .fill(frame_fill)
                        .stroke(egui::Stroke::new(2.0, frame_stroke))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(26, 22)),
                )
                .show(ctx, |ui| {
                    ui.multiply_opacity(0.35 + 0.65 * open_e);
                    ui.add_space((1.0 - open_e) * 6.0);
                    ui.set_width(368.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("在浏览器中完成登录")
                                .size(24.0)
                                .family(display_family())
                                .color(t::ink()),
                        );
                        ui.add_space(6.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(
                                    "登录成功后会自动回到启动器，不需要手动复制代码。",
                                )
                                .size(13.0)
                                .color(t::sub()),
                            )
                            .wrap(),
                        );
                        ui.add_space(14.0);
                        egui::Frame::new()
                            .fill(t::bg())
                            .stroke(egui::Stroke::new(1.0, t::hairline()))
                            .corner_radius(motion::R)
                            .inner_margin(egui::Margin::symmetric(14, 11))
                            .show(ui, |ui| {
                                ui.set_width(310.0);
                                ui.horizontal(|ui| {
                                    busy_spinner(ui);
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new("等待微软授权…")
                                            .size(13.0)
                                            .color(t::sub()),
                                    );
                                });
                            });
                        ui.add_space(14.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(188.0, 34.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                if accent_outline(ui, "重新打开浏览器").clicked() {
                                    open = true;
                                }
                                ui.add_space(6.0);
                                if ghost_button(ui, "取消").clicked() {
                                    cancel_login = true;
                                }
                            },
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("浏览器没有自动打开？点上面的按钮再试一次")
                                .size(13.0)
                                .color(t::sub()),
                        );
                    });
                });
            if modal.backdrop_response.clicked() || escape_pressed(ctx) {
                cancel_login = true;
            }
            if open {
                open_url(&url);
            }
            if cancel_login {
                // 登录和安装共用的是一个“前台任务取消”标志；登录窗口的取消
                // 只是关闭等待，不能让之后的内容浏览误判成“操作已取消”。
                cancel::reset();
                self.login.url = None;
                self.login.rx = None;
                self.login.busy = false;
                self.login.error = None;
                self.notice = Some("已取消登录".into());
            }
        } else if let Some(message) = self.login.error.clone() {
            let mut close = false;
            let mut retry = false;
            let modal = egui::Modal::new(egui::Id::unique("login_error_modal"))
                .backdrop_color(veil)
                .frame(
                    egui::Frame::new()
                        .fill(frame_fill)
                        .stroke(egui::Stroke::new(2.0, frame_stroke))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(26, 22)),
                )
                .show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.label(
                        egui::RichText::new("登录没有完成")
                            .size(24.0)
                            .family(display_family())
                            .color(t::ink()),
                    );
                    ui.add_space(4.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "浏览器里的授权没有回到启动器。可以重试，或稍后再登录。",
                            )
                            .size(13.0)
                            .color(t::sub()),
                        )
                        .wrap(),
                    );
                    ui.add_space(12.0);
                    egui::Frame::new()
                        .fill(t::tint(t::warn(), 0.11))
                        .stroke(egui::Stroke::new(1.0, t::tint(t::warn(), 0.41)))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_width(332.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&message).size(13.0).color(t::warn()),
                                )
                                .wrap(),
                            );
                        });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if motion::primary_sized(ui, "重新登录", true, 104.0, 32.0, 11.5) {
                                retry = true;
                            }
                            ui.add_space(6.0);
                            if motion::secondary_sized(ui, "关闭", true, 72.0, 32.0, 11.0).clicked()
                            {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new("原始错误已写入「设置 › 诊断 › 日志」")
                            .size(13.0)
                            .color(t::sub()),
                    );
                });
            if modal.backdrop_response.clicked() || escape_pressed(ctx) {
                close = true;
            }
            if retry {
                self.login.error = None;
                self.start_login();
            } else if close {
                self.login.error = None;
                self.login.open = false;
            }
        } else if self.login.open
            && let Some(account) = &self.account
        {
            let name = account.name.clone();
            let uuid = account.uuid.clone();
            let mut logout = false;
            let mut close = false;
            let modal = egui::Modal::new(egui::Id::unique("account_modal"))
                .backdrop_color(veil)
                .frame(
                    egui::Frame::new()
                        .fill(frame_fill)
                        .stroke(egui::Stroke::new(2.0, frame_stroke))
                        .corner_radius(motion::R)
                        .inner_margin(egui::Margin::symmetric(24, 20)),
                )
                .show(ctx, |ui| {
                    ui.multiply_opacity(0.35 + 0.65 * open_e);
                    ui.add_space((1.0 - open_e) * 6.0);
                    ui.set_width(360.0);
                    ui.horizontal(|ui| {
                        avatar_sized(ui, &name, 46.0);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&name)
                                    .size(24.0)
                                    .family(display_family()),
                            );
                            ui.horizontal(|ui| {
                                status_dot(ui, true);
                                ui.label(
                                    egui::RichText::new("Microsoft 账户 · 已登录")
                                        .size(13.0)
                                        .color(t::sub()),
                                );
                            });
                        });
                    });
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("UUID").size(13.0).color(t::sub()));
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(short_text(&uuid, 32))
                                    .size(13.0)
                                    .color(t::ink()),
                            )
                            .truncate(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if action_link(ui, "复制").clicked() {
                                ui.ctx().copy_text(uuid.clone());
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("启动时自动使用这个账户，令牌过期会自动续期")
                            .size(13.0)
                            .color(t::sub()),
                    );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("退出登录")
                                        .size(13.0)
                                        .color(t::danger()),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            logout = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary_wide(ui, "关闭", true, 96.0) {
                                close = true;
                            }
                        });
                    });
                });
            if modal.backdrop_response.clicked() || escape_pressed(ctx) {
                close = true;
            }
            if logout {
                self.logout();
            }
            if close {
                self.login.open = false;
            }
        }
    }

    pub(crate) fn start_login(&mut self) {
        if self.login.busy {
            return;
        }
        cancel::reset();
        self.cancel_requested = false;
        self.login.busy = true;
        self.login.url = None;
        self.login.error = None;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.login.rx = Some(rx);
        std::thread::spawn(move || {
            let result = auth::start_auth_code().and_then(|flow| {
                let url = flow.url.clone();
                let _ = tx.send(Msg::LoginBrowser(url.clone()));
                open_url(&url);
                auth::wait_for_code(&flow, || false)
                    .and_then(|tokens| auth::login_with_msa(&tokens))
            });
            let _ = tx.send(Msg::LoginDone(result.map_err(|e| e.to_string())));
        });
    }

    pub(crate) fn logout(&mut self) {
        if let Err(e) = auth::clear(&self.root) {
            self.error = Some(e.to_string());
        }
        self.account = None;
        self.login.open = false;
        self.notice = Some("已退出登录".into());
    }
}

pub(crate) fn avatar_sized(ui: &mut egui::Ui, name: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size * 0.5, t::wash());
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.46),
        t::sub(),
    );
}

/// 登录失败给玩家一句能行动的话；原始 AADSTS 留在设置里的日志。
pub(crate) fn friendly_login_error(raw: &str) -> String {
    if raw.contains("AADSTS900144") {
        "登录参数不完整，请更新到最新版本后重试".to_string()
    } else if raw.contains("invalid_client") || raw.contains("AADSTS70002") {
        "启动器的微软登录配置暂时不可用，请稍后重试".to_string()
    } else if raw.contains("未拥有") || raw.contains("Game Pass") {
        raw.to_string()
    } else if raw.contains("已取消") {
        "已取消登录".to_string()
    } else {
        "登录失败，请重试；详细信息已写入日志".to_string()
    }
}

/// Windows 的浏览器启动参数：URL 必须整体作为一个参数，不能经过 shell。
#[cfg(any(target_os = "windows", test))]
pub(crate) fn browser_launch_args(url: &str) -> [String; 2] {
    ["url.dll,FileProtocolHandler".to_string(), url.to_string()]
}

pub(crate) fn open_url(url: &str) {
    // 不要用 `cmd /C start`：cmd 会把 URL 里的 `&` 当命令分隔符，登录链接
    // 从第一个 `&` 就被截断，微软会报 AADSTS900144（scope 丢失）。
    // rundll32 直接接收完整 URL，不经过 shell。
    #[cfg(target_os = "windows")]
    {
        let args = browser_launch_args(url);
        let _ = std::process::Command::new("rundll32").args(args).spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// 账户弹窗和正在进行的微软登录。
pub(crate) struct LoginState {
    pub(crate) open: bool,
    pub(crate) url: Option<String>,
    pub(crate) rx: Option<mpsc::Receiver<Msg>>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}
