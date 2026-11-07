//! 添加账户（Microsoft / 外置登录 / 离线）和外置账户的重新登录。
//!
//! 外置登录的密码只在登录那一下用，登录成功就从内存里清掉，从不写盘。

use crate::prelude::*;
use nailong_core::accounts::{Account, Login};
use nailong_core::yggdrasil;

/// 添加账户弹窗的三种方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum AddTab {
    #[default]
    Microsoft,
    External,
    Offline,
}

/// 外置登录拿到了凭证，但这个号有好几个角色，等玩家选。
pub(crate) struct Pending {
    pub(crate) server: yggdrasil::Server,
    pub(crate) login: String,
    pub(crate) access_token: String,
    pub(crate) client_token: String,
    pub(crate) profiles: Vec<yggdrasil::Profile>,
}

/// 添加（或重新登录）账户的弹窗。
#[derive(Default)]
pub(crate) struct AddForm {
    pub(crate) tab: AddTab,
    pub(crate) server_input: String,
    /// 已经检查过的输入；输入变了就要重新检查。
    pub(crate) checked_input: String,
    pub(crate) server: Option<yggdrasil::Server>,
    pub(crate) checking: bool,
    pub(crate) server_error: Option<String>,
    pub(crate) login: String,
    pub(crate) password: String,
    pub(crate) offline_name: String,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
    pub(crate) pending: Option<Pending>,
    /// 重新登录这个账户：服务器和账号已经填好，不能改。
    pub(crate) relogin: Option<String>,
    /// 为什么要重新登录（凭证失效之类）。
    pub(crate) reason: Option<String>,
    /// 检查完服务器接着登录。
    pub(crate) login_after_check: bool,
    /// 下一帧检查一次服务器（地址是别处填好的）。
    pub(crate) check_now: bool,
}

impl App {
    /// 打开添加账户；`tab` 是先显示哪种。
    pub(crate) fn open_add_account(&mut self, tab: AddTab) {
        self.login.open = false;
        self.login.add = Some(AddForm {
            tab,
            ..Default::default()
        });
    }

    pub(crate) fn check_server(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.login.add else {
            return;
        };
        let input = form.server_input.trim().to_string();
        if input.is_empty() || form.checking {
            return;
        }
        form.checking = true;
        form.server_error = None;
        let tx = self.login.jobs_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = yggdrasil::resolve(&input).map_err(|e| e.to_string());
            let _ = tx.send(Msg::ServerChecked(input, result));
            ctx.request_repaint();
        });
    }

    pub(crate) fn submit_external(&mut self) {
        let Some(form) = &mut self.login.add else {
            return;
        };
        let Some(server) = form.server.clone() else {
            return;
        };
        if form.login.trim().is_empty() || form.password.is_empty() || form.busy {
            return;
        }
        form.busy = true;
        form.error = None;
        let login = form.login.trim().to_string();
        let password = std::mem::take(&mut form.password);
        let tx = self.login.jobs_tx.clone();
        std::thread::spawn(move || {
            let result = yggdrasil::authenticate(&server.api, &login, &password);
            drop(password);
            let msg = match result {
                Err(e) => Msg::ExternalLogin(Err(e.to_string())),
                Ok(done) => match done.selected.clone() {
                    Some(profile) => Msg::AccountAdded(Ok(Box::new(finish_external(
                        &server,
                        &login,
                        done.access_token,
                        done.client_token,
                        &profile,
                    )))),
                    None if done.profiles.is_empty() => {
                        Msg::ExternalLogin(Err(format!("这个账号在 {} 没有角色", server.name)))
                    }
                    None => Msg::ExternalLogin(Ok(Box::new(Pending {
                        server,
                        login,
                        access_token: done.access_token,
                        client_token: done.client_token,
                        profiles: done.profiles,
                    }))),
                },
            };
            let _ = tx.send(msg);
        });
    }

    pub(crate) fn choose_profile(&mut self, profile: yggdrasil::Profile) {
        let Some(form) = &mut self.login.add else {
            return;
        };
        let Some(pending) = form.pending.take() else {
            return;
        };
        form.busy = true;
        let tx = self.login.jobs_tx.clone();
        std::thread::spawn(move || {
            let result = yggdrasil::select_profile(
                &pending.server.api,
                &pending.access_token,
                &pending.client_token,
                &profile,
            )
            .map(|token| {
                Box::new(finish_external(
                    &pending.server,
                    &pending.login,
                    token,
                    pending.client_token.clone(),
                    &profile,
                ))
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(Msg::AccountAdded(result));
        });
    }

    pub(crate) fn add_offline(&mut self) {
        let Some(form) = &mut self.login.add else {
            return;
        };
        let name = form.offline_name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let account = Account::offline(&name);
        self.accounts.add(account);
        self.save_accounts();
        self.login.add = None;
        self.notice = Some(format!("已添加 {name}"));
    }

    /// 添加账户（或重新登录）的弹窗。
    pub(crate) fn add_modal(&mut self, ctx: &egui::Context) {
        let Some(mut form) = self.login.add.take() else {
            return;
        };
        let known_servers: Vec<(String, String)> = {
            let mut seen = Vec::<(String, String)>::new();
            for account in &self.accounts.list {
                if let Login::External {
                    api, server_name, ..
                } = &account.login
                    && !seen.iter().any(|(a, _)| a == api)
                {
                    seen.push((api.clone(), server_name.clone()));
                }
            }
            seen
        };
        let mut close = false;
        let mut check = false;
        let mut submit_external = false;
        let mut submit_offline = false;
        let mut microsoft = false;
        let mut choose: Option<yggdrasil::Profile> = None;
        let relogin = form.relogin.is_some();
        if std::mem::take(&mut form.check_now) {
            check = true;
        }
        let modal = egui::Modal::new(egui::Id::unique("add_account"))
            .backdrop_color(t::veil(1.0))
            .frame(modal_frame())
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(
                    egui::RichText::new(if relogin {
                        "重新登录"
                    } else {
                        "添加账户"
                    })
                    .size(24.0)
                    .family(display_family()),
                );
                if let Some(reason) = &form.reason {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(reason).size(13.0).color(t::warn()));
                }
                ui.add_space(16.0);
                if !relogin && form.pending.is_none() {
                    ui.horizontal(|ui| {
                        for (tab, label) in [
                            (AddTab::Microsoft, "Microsoft"),
                            (AddTab::External, "外置登录"),
                            (AddTab::Offline, "离线"),
                        ] {
                            if chip(ui, label, form.tab == tab).clicked() {
                                form.tab = tab;
                                form.error = None;
                            }
                        }
                    });
                    ui.add_space(16.0);
                }
                if let Some(pending) = &form.pending {
                    ui.label(
                        egui::RichText::new(format!(
                            "选择角色（{} · {} 个）",
                            pending.server.name,
                            pending.profiles.len()
                        ))
                        .size(13.5)
                        .color(t::sub()),
                    );
                    ui.add_space(8.0);
                    for profile in &pending.profiles {
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 40.0),
                            egui::Sense::click(),
                        );
                        list_row_fill(ui, rect, false, response.hovered());
                        let icon = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + 8.0, rect.center().y - 12.0),
                            egui::vec2(24.0, 24.0),
                        );
                        initial_avatar(ui.painter(), icon, &profile.name);
                        ui.painter().text(
                            egui::pos2(rect.left() + 42.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &profile.name,
                            egui::FontId::proportional(14.5),
                            t::ink(),
                        );
                        if response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            choose = Some(profile.clone());
                        }
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ghost_button(ui, "取消").clicked() {
                            close = true;
                        }
                    });
                    return;
                }
                match form.tab {
                    AddTab::Microsoft => {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("在浏览器中登录微软账户")
                                    .size(13.5)
                                    .color(t::sub()),
                            )
                            .wrap(),
                        );
                        ui.add_space(20.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if motion::primary_sized(ui, "在浏览器中登录", true, 150.0, 34.0, 12.0)
                            {
                                microsoft = true;
                            }
                            if ghost_button(ui, "取消").clicked() {
                                close = true;
                            }
                        });
                    }
                    AddTab::External => {
                        let field_width = ui.available_width();
                        ui.label(egui::RichText::new("认证服务器").size(13.0).color(t::sub()));
                        ui.add_space(4.0);
                        let response = ui.add_enabled(
                            !relogin,
                            egui::TextEdit::singleline(&mut form.server_input)
                                .hint_text("例如 littleskin.cn")
                                .desired_width(field_width - 20.0)
                                .frame(field_frame()),
                        );
                        if response.lost_focus()
                            || ctx.input(|i| i.key_pressed(egui::Key::Enter))
                                && response.has_focus()
                        {
                            check = true;
                        }
                        // 皮肤站「添加到启动器」的链接粘贴进来，换成里面的接口地址。
                        if let Some(api) = yggdrasil::parse_drop(&form.server_input) {
                            form.server_input = api;
                            check = true;
                        }
                        if form.server_input.trim() != form.checked_input && !form.checking {
                            form.server = None;
                            form.server_error = None;
                        }
                        if !known_servers.is_empty() && !relogin {
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new("用过的").size(12.5).color(t::dim()));
                                for (api, name) in &known_servers {
                                    if chip(ui, name, form.server_input.trim() == api.as_str())
                                        .on_hover_text(api)
                                        .clicked()
                                    {
                                        form.server_input = api.clone();
                                        check = true;
                                    }
                                }
                            });
                        }
                        ui.add_space(8.0);
                        if form.checking {
                            ui.horizontal(|ui| {
                                busy_spinner(ui);
                                ui.label(
                                    egui::RichText::new("正在连接…").size(13.0).color(t::sub()),
                                );
                            });
                        } else if let Some(error) = &form.server_error {
                            ui.label(egui::RichText::new(error).size(13.0).color(t::warn()));
                        } else if let Some(server) = &form.server {
                            egui::Frame::new()
                                .fill(t::bg())
                                .stroke(egui::Stroke::new(1.0, t::hairline()))
                                .corner_radius(motion::R)
                                .inner_margin(egui::Margin::symmetric(12, 9))
                                .show(ui, |ui| {
                                    ui.set_width(field_width - 26.0);
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(&server.name)
                                                    .size(15.0)
                                                    .family(bold_family()),
                                            );
                                            ui.label(
                                                egui::RichText::new(short_text(&server.api, 46))
                                                    .size(12.5)
                                                    .color(t::dim()),
                                            )
                                            .on_hover_text(&server.api);
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if let Some(home) = &server.homepage
                                                    && ui.link("主页").clicked()
                                                {
                                                    open_url(home);
                                                }
                                                if let Some(register) = &server.register
                                                    && ui.link("注册").clicked()
                                                {
                                                    open_url(register);
                                                }
                                            },
                                        );
                                    });
                                });
                            if server.insecure {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("未加密连接（http），密码将明文传输")
                                        .size(13.0)
                                        .color(t::danger()),
                                );
                            }
                        }
                        let non_email = form.server.as_ref().is_some_and(|s| s.non_email_login);
                        ui.add_space(14.0);
                        ui.label(
                            egui::RichText::new(if non_email { "账号" } else { "邮箱" })
                                .size(13.0)
                                .color(t::sub()),
                        );
                        ui.add_space(4.0);
                        ui.add_enabled(
                            !relogin,
                            egui::TextEdit::singleline(&mut form.login)
                                .hint_text(if non_email {
                                    "邮箱或用户名"
                                } else {
                                    "邮箱"
                                })
                                .desired_width(field_width - 20.0)
                                .frame(field_frame()),
                        );
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("密码").size(13.0).color(t::sub()));
                        ui.add_space(4.0);
                        let password = ui.add(
                            egui::TextEdit::singleline(&mut form.password)
                                .password(true)
                                .desired_width(field_width - 20.0)
                                .frame(field_frame()),
                        );
                        if relogin
                            && !password.has_focus()
                            && form.password.is_empty()
                            && !form.busy
                        {
                            password.request_focus();
                        }
                        if password.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                            submit_external = true;
                        }
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("不保存密码").size(12.5).color(t::dim()));
                        if let Some(error) = &form.error {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(error).size(13.0).color(t::warn()));
                        }
                        ui.add_space(16.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ready = !form.busy
                                && !form.server_input.trim().is_empty()
                                && !form.login.trim().is_empty()
                                && !form.password.is_empty();
                            let label = if form.busy {
                                "正在登录…"
                            } else {
                                "登录"
                            };
                            if motion::primary_sized(ui, label, ready, 110.0, 34.0, 12.0) {
                                submit_external = true;
                            }
                            if ghost_button(ui, "取消").clicked() {
                                close = true;
                            }
                        });
                    }
                    AddTab::Offline => {
                        let field_width = ui.available_width();
                        ui.label(egui::RichText::new("名字").size(13.0).color(t::sub()));
                        ui.add_space(4.0);
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut form.offline_name)
                                .hint_text("玩家名")
                                .char_limit(32)
                                .desired_width(field_width - 20.0)
                                .frame(field_frame()),
                        );
                        if response.has_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                            submit_offline = true;
                        }
                        let name = form.offline_name.trim();
                        ui.add_space(6.0);
                        if !name.is_empty() && !valid_player_name(name) {
                            ui.label(
                                egui::RichText::new("多数服务器只接受 3–16 位字母、数字或下划线")
                                    .size(12.5)
                                    .color(t::warn()),
                            );
                        } else {
                            ui.label(
                                egui::RichText::new("用于单人、局域网和离线服务器")
                                    .size(12.5)
                                    .color(t::dim()),
                            );
                        }
                        if !name.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(
                                    "UUID {}",
                                    dashed_uuid(&launch::offline_uuid(name))
                                ))
                                .size(12.5)
                                .color(t::dim()),
                            );
                        }
                        ui.add_space(18.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if motion::primary_sized(ui, "添加", !name.is_empty(), 96.0, 34.0, 12.0)
                            {
                                submit_offline = true;
                            }
                            if ghost_button(ui, "取消").clicked() {
                                close = true;
                            }
                        });
                    }
                }
            });
        let busy = form.busy || form.checking;
        if (modal.backdrop_response.clicked() || escape_pressed(ctx)) && !busy {
            close = true;
        }
        if close {
            form.password.clear();
            return;
        }
        self.login.add = Some(form);
        if check {
            self.check_server(ctx);
        }
        if microsoft {
            self.login.add = None;
            self.start_login();
        }
        if submit_offline {
            self.add_offline();
        }
        if let Some(profile) = choose {
            self.choose_profile(profile);
        }
        if submit_external {
            let needs_check =
                self.login.add.as_ref().is_some_and(|f| {
                    f.server.is_none() || f.server_input.trim() != f.checked_input
                });
            if needs_check {
                if let Some(form) = &mut self.login.add {
                    form.login_after_check = true;
                }
                self.check_server(ctx);
            } else {
                self.submit_external();
            }
        }
    }
}

/// 外置账户登录完，顺手取一下皮肤地址。
pub(crate) fn finish_external(
    server: &yggdrasil::Server,
    login: &str,
    access_token: String,
    client_token: String,
    profile: &yggdrasil::Profile,
) -> Account {
    let mut account = Account::external(server, login, access_token, client_token, profile);
    account.skin = yggdrasil::skin_url(&server.api, &profile.id);
    account
}

pub(crate) fn field_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(1.0, t::control()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(10, 7))
}

/// Mojang 对正版名字的规矩：3–16 个英文字母、数字或下划线。
pub(crate) fn valid_player_name(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_player_names() {
        assert!(valid_player_name("Steve_01"));
        assert!(!valid_player_name("史蒂夫"));
        assert!(!valid_player_name("ab"));
    }
}
