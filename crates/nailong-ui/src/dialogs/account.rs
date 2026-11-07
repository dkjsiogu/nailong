//! 账户：左下角的账户列表（点一下就换）、添加账户（Microsoft / 外置登录 / 离线）、
//! 微软的浏览器登录，以及皮肤头像。
//!
//! 账户存在 `accounts.json`，界面上的增删换都立即落盘。外置登录的密码只在登录那一下用，
//! 登录成功就从内存里清掉，从不写盘。

use crate::prelude::*;
use nailong_core::accounts::{Account, Accounts, Login};
use nailong_core::yggdrasil;

/// 头像：皮肤的脸。
pub(crate) enum Face {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
}

/// 账户列表、添加账户、微软登录和头像。
pub(crate) struct LoginState {
    /// 左下角的账户列表是否展开。
    pub(crate) open: bool,
    /// 账户列表从哪里弹出来（侧栏底部那一行）。
    pub(crate) anchor: Option<egui::Rect>,
    /// 微软登录：浏览器授权地址。
    pub(crate) url: Option<String>,
    pub(crate) rx: Option<mpsc::Receiver<Msg>>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
    pub(crate) add: Option<AddForm>,
    /// 等确认移除的账户。
    pub(crate) remove: Option<String>,
    /// 账户相关的后台结果都回到这里。
    pub(crate) jobs_tx: mpsc::Sender<Msg>,
    pub(crate) jobs_rx: mpsc::Receiver<Msg>,
    /// 皮肤地址 → 脸。
    pub(crate) faces: HashMap<String, Face>,
}

impl Default for LoginState {
    fn default() -> Self {
        let (jobs_tx, jobs_rx) = mpsc::channel();
        Self {
            open: false,
            anchor: None,
            url: None,
            rx: None,
            busy: false,
            error: None,
            add: None,
            remove: None,
            jobs_tx,
            jobs_rx,
            faces: HashMap::new(),
        }
    }
}

pub(crate) fn modal_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(t::surface())
        .stroke(egui::Stroke::new(2.0, t::hairline()))
        .corner_radius(motion::R)
        .inner_margin(egui::Margin::symmetric(26, 22))
}

impl App {
    /// 正在用的账户。
    pub(crate) fn current_account(&self) -> Option<&Account> {
        self.accounts.current()
    }

    pub(crate) fn save_accounts(&mut self) {
        if let Err(e) = self.accounts.save(&self.root) {
            self.error = Some(format!("账户没能保存：{e}"));
        }
    }

    pub(crate) fn select_account(&mut self, id: &str) {
        if self.accounts.select(id) {
            self.save_accounts();
            if let Some(account) = self.accounts.current() {
                self.notice = Some(format!("已切换到 {}", account.name));
            }
        }
    }

    /// 某个账户要重新登录：微软走浏览器，外置再输一次密码，离线不需要。
    pub(crate) fn relogin(&mut self, id: &str, reason: Option<String>) {
        let Some(account) = self.accounts.get(id).cloned() else {
            return;
        };
        self.login.open = false;
        match &account.login {
            Login::Microsoft { .. } => {
                if let Some(reason) = reason {
                    self.notice = Some(format!("{}：{reason}", account.name));
                }
                self.start_login();
            }
            Login::External {
                api,
                server_name,
                login,
                ..
            } => {
                let server = yggdrasil::Server {
                    api: api.clone(),
                    name: server_name.clone(),
                    homepage: None,
                    register: None,
                    non_email_login: !login.contains('@'),
                    insecure: api.starts_with("http://"),
                    metadata: String::new(),
                };
                self.login.add = Some(AddForm {
                    tab: AddTab::External,
                    server_input: api.clone(),
                    checked_input: api.clone(),
                    server: Some(server),
                    login: login.clone(),
                    relogin: Some(account.id.clone()),
                    reason,
                    ..Default::default()
                });
            }
            Login::Offline => {}
        }
    }

    /// 后台的账户结果：检查服务器、登录、选角色、头像。
    pub(crate) fn poll_accounts(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.login.jobs_rx.try_recv() {
            match msg {
                Msg::ServerChecked(input, result) => {
                    let Some(form) = &mut self.login.add else {
                        continue;
                    };
                    if form.server_input.trim() != input {
                        continue;
                    }
                    form.checking = false;
                    form.checked_input = input;
                    match result {
                        Ok(server) => {
                            form.server = Some(server);
                            form.server_error = None;
                            if std::mem::take(&mut form.login_after_check) {
                                self.submit_external();
                            }
                        }
                        Err(e) => {
                            form.server = None;
                            form.server_error = Some(e);
                            form.login_after_check = false;
                        }
                    }
                }
                Msg::ExternalLogin(result) => {
                    let Some(form) = &mut self.login.add else {
                        continue;
                    };
                    form.busy = false;
                    match result {
                        Ok(pending) => {
                            form.password.clear();
                            form.error = None;
                            form.pending = Some(*pending);
                        }
                        Err(e) => form.error = Some(e),
                    }
                }
                Msg::AccountAdded(result) => match result {
                    Ok(account) => {
                        let name = account.name.clone();
                        self.accounts.add(*account);
                        self.save_accounts();
                        self.login.add = None;
                        self.notice = Some(format!("已添加 {name}"));
                    }
                    Err(e) => {
                        if let Some(form) = &mut self.login.add {
                            form.busy = false;
                            form.error = Some(e);
                        }
                    }
                },
                Msg::Face(url, image) => {
                    let face = match image {
                        Some(image) => Face::Ready(ctx.load_texture(
                            format!("face:{url}"),
                            image,
                            egui::TextureOptions::NEAREST,
                        )),
                        None => Face::Missing,
                    };
                    self.login.faces.insert(url, face);
                }
                _ => {}
            }
        }
    }

    /// 皮肤的脸；还没有就在后台去取，先返回 None（画首字母）。
    pub(crate) fn face(
        &mut self,
        ctx: &egui::Context,
        skin: Option<&str>,
    ) -> Option<egui::TextureHandle> {
        let url = skin?;
        match self.login.faces.get(url) {
            Some(Face::Ready(texture)) => return Some(texture.clone()),
            Some(_) => return None,
            None => {}
        }
        self.login.faces.insert(url.to_string(), Face::Loading);
        let url = url.to_string();
        let cache = self.root.cache().join("skins");
        let tx = self.login.jobs_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let image = load_skin(&cache, &url).and_then(|bytes| face_from_skin(&bytes));
            let _ = tx.send(Msg::Face(url, image));
            ctx.request_repaint();
        });
        None
    }

    pub(crate) fn account_overlay(&mut self, ctx: &egui::Context) {
        self.account_menu(ctx);
        if self.login.url.is_some() {
            self.browser_login_modal(ctx);
        } else if self.login.error.is_some() {
            self.login_error_modal(ctx);
        } else if self.login.remove.is_some() {
            self.remove_modal(ctx);
        } else if self.login.add.is_some() {
            self.add_modal(ctx);
        }
    }

    /// 左下角弹出的账户列表。
    fn account_menu(&mut self, ctx: &egui::Context) {
        if !self.login.open {
            return;
        }
        let Some(anchor) = self.login.anchor else {
            self.login.open = false;
            return;
        };
        let list: Vec<Account> = self.accounts.list.clone();
        let current = self.accounts.current.clone();
        let faces: Vec<Option<egui::TextureHandle>> = list
            .iter()
            .map(|a| self.face(ctx, a.skin.as_deref()))
            .collect();
        let mut pick: Option<String> = None;
        let mut add = false;
        let mut relogin: Option<String> = None;
        let mut remove: Option<String> = None;
        let area = egui::Area::new(egui::Id::unique("account_menu"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(egui::pos2(anchor.left(), anchor.top() - 6.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t::surface())
                    .stroke(egui::Stroke::new(1.0, t::hairline()))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(8))
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(284.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(if list.len() > 1 {
                                    "切换账户"
                                } else {
                                    "账户"
                                })
                                .size(12.5)
                                .color(t::dim()),
                            );
                        });
                        ui.add_space(4.0);
                        for (account, face) in list.iter().zip(&faces) {
                            let is_current = current.as_deref() == Some(account.id.as_str());
                            let row = account_row(ui, account, face.as_ref(), is_current);
                            if row.clicked {
                                pick = Some(account.id.clone());
                            }
                            let mut chosen = egui::Popup::menu(&row.dots)
                                .show(|ui| account_menu_items(ui, account))
                                .and_then(|inner| inner.inner);
                            row.response.context_menu(|ui| {
                                if let Some(action) = account_menu_items(ui, account) {
                                    chosen = Some(action);
                                }
                            });
                            match chosen {
                                Some(MenuAction::Relogin) => relogin = Some(account.id.clone()),
                                Some(MenuAction::Remove) => remove = Some(account.id.clone()),
                                None => {}
                            }
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(2.0);
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 42.0),
                            egui::Sense::click(),
                        );
                        list_row_fill(ui, rect, false, response.hovered());
                        let icon = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + 10.0, rect.center().y - 14.0),
                            egui::vec2(28.0, 28.0),
                        );
                        ui.painter().rect_filled(icon, motion::R, t::wash());
                        ui.painter().text(
                            icon.center(),
                            egui::Align2::CENTER_CENTER,
                            "+",
                            egui::FontId::proportional(18.0),
                            t::sub(),
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 48.0, rect.center().y - 8.0),
                            egui::Align2::LEFT_CENTER,
                            "添加账户",
                            egui::FontId::proportional(14.0),
                            t::ink(),
                        );
                        ui.painter().text(
                            egui::pos2(rect.left() + 48.0, rect.center().y + 10.0),
                            egui::Align2::LEFT_CENTER,
                            "Microsoft · 外置登录 · 离线",
                            egui::FontId::proportional(12.5),
                            t::dim(),
                        );
                        if response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            add = true;
                        }
                    });
            });
        let popup_open = egui::Popup::is_any_open(ctx);
        let pointer_in_anchor = ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| anchor.contains(p));
        if (area.response.clicked_elsewhere() && !pointer_in_anchor && !popup_open)
            || escape_pressed(ctx)
        {
            self.login.open = false;
        }
        if let Some(id) = pick {
            self.login.open = false;
            let stale = self.accounts.get(&id).is_some_and(|a| a.stale);
            self.select_account(&id);
            if stale {
                self.relogin(&id, Some("登录已失效".into()));
            }
        }
        if add {
            self.open_add_account(AddTab::Microsoft);
        }
        if let Some(id) = relogin {
            self.relogin(&id, None);
        }
        if let Some(id) = remove {
            self.login.open = false;
            self.login.remove = Some(id);
        }
    }

    fn remove_modal(&mut self, ctx: &egui::Context) {
        let Some(id) = self.login.remove.clone() else {
            return;
        };
        let Some(account) = self.accounts.get(&id).cloned() else {
            self.login.remove = None;
            return;
        };
        let mut confirm = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("remove_account"))
            .backdrop_color(t::veil(1.0))
            .frame(modal_frame())
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(
                    egui::RichText::new(format!("移除 {}？", account.name))
                        .size(22.0)
                        .family(display_family()),
                );
                if !account.is_offline() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("移除后需要重新登录")
                            .size(13.0)
                            .color(t::sub()),
                    );
                }
                ui.add_space(16.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("移除").color(t::danger()).size(13.5),
                            )
                            .stroke(egui::Stroke::new(1.0, t::tint(t::danger(), 0.6)))
                            .fill(egui::Color32::TRANSPARENT)
                            .corner_radius(motion::R)
                            .min_size(egui::vec2(80.0, 32.0)),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ghost_button(ui, "取消").clicked() {
                        close = true;
                    }
                });
            });
        if modal.backdrop_response.clicked() || escape_pressed(ctx) {
            close = true;
        }
        if confirm {
            self.accounts.remove(&id);
            self.save_accounts();
            self.login.remove = None;
            self.notice = Some(format!("已移除 {}", account.name));
        } else if close {
            self.login.remove = None;
        }
    }

    fn browser_login_modal(&mut self, ctx: &egui::Context) {
        let Some(url) = self.login.url.clone() else {
            return;
        };
        let mut open = false;
        let mut cancel_login = false;
        let modal = egui::Modal::new(egui::Id::unique("login_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(modal_frame())
            .show(ctx, |ui| {
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
                            egui::RichText::new("登录后自动返回")
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
                        egui::RichText::new("浏览器没有打开时，点上面的按钮")
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
    }

    fn login_error_modal(&mut self, ctx: &egui::Context) {
        let Some(message) = self.login.error.clone() else {
            return;
        };
        let mut close = false;
        let mut retry = false;
        let modal = egui::Modal::new(egui::Id::unique("login_error_modal"))
            .backdrop_color(t::veil(1.0))
            .frame(modal_frame())
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
                        egui::RichText::new("浏览器授权未完成")
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
                        if motion::secondary_sized(ui, "关闭", true, 72.0, 32.0, 11.0).clicked() {
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
        self.login.open = false;
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
}

/// 账户列表里的一行。
struct AccountRow {
    response: egui::Response,
    clicked: bool,
    dots: egui::Response,
}

/// 账户的「更多」菜单里需要回到 App 处理的操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Relogin,
    Remove,
}

/// 「更多」菜单（⋯ 和右键是同一份）。复制和打开网页当场做完。
fn account_menu_items(ui: &mut egui::Ui, account: &Account) -> Option<MenuAction> {
    ui.set_min_width(176.0);
    let mut action = None;
    if ui.button("复制名字").clicked() {
        ui.ctx().copy_text(account.name.clone());
    }
    if ui.button("复制 UUID").clicked() {
        ui.ctx().copy_text(dashed_uuid(&account.uuid));
    }
    if let Login::External {
        api, server_name, ..
    } = &account.login
        && ui
            .button(format!("打开 {server_name}"))
            .on_hover_text(api.as_str())
            .clicked()
    {
        open_url(&site_of(api));
    }
    if !account.is_offline() && ui.button("重新登录").clicked() {
        action = Some(MenuAction::Relogin);
    }
    ui.separator();
    if ui
        .button(egui::RichText::new("移除").color(t::danger()))
        .clicked()
    {
        action = Some(MenuAction::Remove);
    }
    action
}

fn account_row(
    ui: &mut egui::Ui,
    account: &Account,
    face: Option<&egui::TextureHandle>,
    current: bool,
) -> AccountRow {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::click());
    let hovered = response.hovered() || response.context_menu_opened();
    list_row_fill(ui, rect, false, hovered);
    let icon = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 10.0, rect.center().y - 14.0),
        egui::vec2(28.0, 28.0),
    );
    draw_avatar(ui.painter(), icon, &account.name, face);
    let text_left = rect.left() + 48.0;
    ui.painter().text(
        egui::pos2(text_left, rect.center().y - 9.0),
        egui::Align2::LEFT_CENTER,
        short_text(&account.name, 18),
        egui::FontId::proportional(14.0),
        t::ink(),
    );
    let (sub, color) = if account.stale {
        ("● 需要重新登录".to_string(), t::warn())
    } else {
        (account.kind_label(), t::dim())
    };
    ui.painter().text(
        egui::pos2(text_left, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        short_text(&sub, 22),
        egui::FontId::proportional(12.5),
        color,
    );
    let dots_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        egui::vec2(26.0, 26.0),
    );
    let dots = ui
        .put(
            dots_rect,
            egui::Button::new(egui::RichText::new("⋯").size(15.0).color(if hovered {
                t::sub()
            } else {
                t::dim()
            }))
            .frame(false),
        )
        .on_hover_text("更多");
    if current {
        ui.painter().text(
            egui::pos2(rect.right() - 38.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "● 正在用",
            egui::FontId::proportional(12.5),
            t::accent(),
        );
    }
    let clicked = response.clicked() && !dots.hovered();
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    AccountRow {
        response,
        clicked,
        dots,
    }
}

/// 头像：有皮肤画脸，没有画首字母。
pub(crate) fn draw_avatar(
    painter: &egui::Painter,
    rect: egui::Rect,
    name: &str,
    face: Option<&egui::TextureHandle>,
) {
    match face {
        Some(texture) => {
            painter.add(
                egui::epaint::RectShape::filled(rect, motion::R, egui::Color32::WHITE)
                    .with_texture(
                        texture.id(),
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    ),
            );
        }
        None => initial_avatar(painter, rect, name),
    }
}

pub(crate) fn initial_avatar(painter: &egui::Painter, rect: egui::Rect, name: &str) {
    painter.rect_filled(rect, motion::R, t::wash());
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(rect.height() * 0.5),
        t::sub(),
    );
}

/// 无短横线的 UUID 加上短横线，方便贴到别处。
pub(crate) fn dashed_uuid(uuid: &str) -> String {
    if uuid.len() != 32 {
        return uuid.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &uuid[..8],
        &uuid[8..12],
        &uuid[12..16],
        &uuid[16..20],
        &uuid[20..]
    )
}

/// 接口地址所在的网站首页：`https://littleskin.cn/api/yggdrasil/` → `https://littleskin.cn/`。
fn site_of(api: &str) -> String {
    let (scheme, rest) = api.split_once("://").unwrap_or(("https", api));
    let host = rest.split('/').next().unwrap_or(rest);
    format!("{scheme}://{host}/")
}

/// 读皮肤图片：先看本地缓存，没有再下载。
fn load_skin(cache: &std::path::Path, url: &str) -> Option<Vec<u8>> {
    // 调试构建里可以用本地皮肤，方便离线截图核对。
    #[cfg(debug_assertions)]
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).ok();
    }
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    let path = cache.join(format!("{:016x}.png", hasher.finish()));
    if let Ok(bytes) = std::fs::read(&path) {
        return Some(bytes);
    }
    let http = nailong_core::http::Http::new("official").ok()?;
    let bytes = http.get_bytes(url).ok()?;
    let _ = std::fs::create_dir_all(cache);
    let _ = std::fs::write(&path, &bytes);
    Some(bytes)
}

/// 从皮肤图里切出脸（8×8），盖上帽子那一层。高清皮肤按比例取。
pub(crate) fn face_from_skin(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    if image.width() < 64 || image.height() < 32 || image.width() % 64 != 0 {
        return None;
    }
    let scale = image.width() / 64;
    let mut pixels = Vec::with_capacity(64);
    for y in 0..8 {
        for x in 0..8 {
            let base = image.get_pixel((8 + x) * scale, (8 + y) * scale).0;
            let over = image.get_pixel((40 + x) * scale, (8 + y) * scale).0;
            let a = over[3] as u32;
            let mix = |i: usize| ((over[i] as u32 * a + base[i] as u32 * (255 - a)) / 255) as u8;
            pixels.push(egui::Color32::from_rgb(mix(0), mix(1), mix(2)));
        }
    }
    Some(egui::ColorImage::new([8, 8], pixels))
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
        "登录失败，详情见日志".to_string()
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

/// 第一次打开 0.40：把以前设置里的玩家名变成一个离线账户（还没有任何账户时）。
pub(crate) fn load_accounts(root: &Root, legacy_name: &str) -> Accounts {
    let mut accounts = Accounts::load(root);
    if accounts.list.is_empty() {
        accounts.add(Account::offline(legacy_name));
        let _ = accounts.save(root);
    }
    if accounts.current().is_none()
        && let Some(first) = accounts.list.first().map(|a| a.id.clone())
    {
        accounts.select(&first);
    }
    accounts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_the_face_out_of_a_skin() {
        let mut skin = image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 20, 30, 255]));
        skin.put_pixel(8, 8, image::Rgba([200, 0, 0, 255]));
        // 帽子层半透明地盖在 (9, 8) 上。
        skin.put_pixel(41, 8, image::Rgba([0, 0, 255, 255]));
        for x in 40..48 {
            for y in 8..16 {
                if (x, y) != (41, 8) {
                    skin.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                }
            }
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(skin)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let face = face_from_skin(&png).unwrap();
        assert_eq!(face.size, [8, 8]);
        assert_eq!(face.pixels[0], egui::Color32::from_rgb(200, 0, 0));
        assert_eq!(face.pixels[1], egui::Color32::from_rgb(0, 0, 255));
        assert_eq!(face.pixels[2], egui::Color32::from_rgb(10, 20, 30));
        assert!(face_from_skin(b"not a png").is_none());
    }

    #[test]
    fn formats_uuids_and_names() {
        assert_eq!(
            dashed_uuid("069a79f444e94726a5befca90e38aaf5"),
            "069a79f4-44e9-4726-a5be-fca90e38aaf5"
        );
        assert_eq!(
            site_of("https://littleskin.cn/api/yggdrasil/"),
            "https://littleskin.cn/"
        );
    }
}
