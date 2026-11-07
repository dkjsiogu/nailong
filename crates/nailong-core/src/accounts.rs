//! 账户：同时留着好几个——微软正版、外置登录（authlib-injector）、离线——左下角一点就换。
//!
//! 全部存在数据目录的 `accounts.json`（Unix 上权限 600）。只存登录凭证，**从不存密码**。
//! 0.40 之前只有一个微软账户，存在 `account.json`；第一次读的时候搬过来，旧文件删掉，
//! 免得凭证留两份。

use crate::auth;
use crate::error::{Error, Result};
use crate::paths::Root;
use crate::yggdrasil;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 账户是怎么登录的，以及它的凭证。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Login {
    Microsoft {
        access_token: String,
        refresh_token: String,
        /// Unix 秒，Minecraft 令牌过期的时间。
        expires_at: u64,
        xuid: Option<String>,
        client_id: String,
    },
    External {
        /// 接口地址（已经过 ALI 解析），以 `/` 结尾。
        api: String,
        /// 服务器自己报的名字，例如 LittleSkin。
        server_name: String,
        /// 登录用的邮箱或用户名。重新登录时帮玩家填好。
        login: String,
        access_token: String,
        client_token: String,
    },
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// 本地编号，换名字也不变。
    pub id: String,
    /// 游戏里的名字。
    pub name: String,
    /// 角色 UUID，无短横线。
    pub uuid: String,
    pub login: Login,
    /// 皮肤图片地址；没有就显示首字母。
    #[serde(default)]
    pub skin: Option<String>,
    /// 凭证已经失效，要重新登录：外置账户再输一次密码，微软再走一次浏览器。
    #[serde(default)]
    pub stale: bool,
}

impl Account {
    pub fn offline(name: &str) -> Self {
        Self {
            id: new_id(),
            name: name.to_string(),
            uuid: crate::launch::offline_uuid(name),
            login: Login::Offline,
            skin: None,
            stale: false,
        }
    }

    pub fn from_microsoft(account: auth::Account) -> Self {
        Self {
            id: new_id(),
            name: account.name,
            uuid: account.uuid,
            skin: account.skin,
            stale: false,
            login: Login::Microsoft {
                access_token: account.access_token,
                refresh_token: account.refresh_token,
                expires_at: account.expires_at,
                xuid: account.xuid,
                client_id: account.client_id,
            },
        }
    }

    pub fn external(
        server: &yggdrasil::Server,
        login: &str,
        access_token: String,
        client_token: String,
        profile: &yggdrasil::Profile,
    ) -> Self {
        Self {
            id: new_id(),
            name: profile.name.clone(),
            uuid: profile.id.clone(),
            skin: None,
            stale: false,
            login: Login::External {
                api: server.api.clone(),
                server_name: server.name.clone(),
                login: login.to_string(),
                access_token,
                client_token,
            },
        }
    }

    /// 一行说明：「Microsoft 正版」「LittleSkin · 外置登录」「离线」。
    pub fn kind_label(&self) -> String {
        match &self.login {
            Login::Microsoft { .. } => "Microsoft 正版".into(),
            Login::External { server_name, .. } => format!("{server_name} · 外置登录"),
            Login::Offline => "离线".into(),
        }
    }

    /// 短的类型名，写进截图：「正版」「LittleSkin」「离线」。
    pub fn kind_short(&self) -> String {
        match &self.login {
            Login::Microsoft { .. } => "正版".into(),
            Login::External { server_name, .. } => server_name.clone(),
            Login::Offline => "离线".into(),
        }
    }

    pub fn is_offline(&self) -> bool {
        matches!(self.login, Login::Offline)
    }

    /// 同一个人：微软看 UUID；外置看服务器和 UUID；离线看名字（不分大小写）。
    pub fn same_as(&self, other: &Account) -> bool {
        match (&self.login, &other.login) {
            (Login::Microsoft { .. }, Login::Microsoft { .. }) => self.uuid == other.uuid,
            (Login::External { api: a, .. }, Login::External { api: b, .. }) => {
                a == b && self.uuid == other.uuid
            }
            (Login::Offline, Login::Offline) => self.name.eq_ignore_ascii_case(&other.name),
            _ => false,
        }
    }

    fn to_microsoft(&self) -> Option<auth::Account> {
        match &self.login {
            Login::Microsoft {
                access_token,
                refresh_token,
                expires_at,
                xuid,
                client_id,
            } => Some(auth::Account {
                name: self.name.clone(),
                uuid: self.uuid.clone(),
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                expires_at: *expires_at,
                xuid: xuid.clone(),
                client_id: client_id.clone(),
                skin: self.skin.clone(),
            }),
            _ => None,
        }
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 全部账户和正在用的那个。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accounts {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub list: Vec<Account>,
}

fn store_path(root: &Root) -> PathBuf {
    root.dir().join("accounts.json")
}

impl Accounts {
    /// 读账户；第一次读时把 0.40 之前的 `account.json` 搬过来。
    pub fn load(root: &Root) -> Self {
        let path = store_path(root);
        if let Ok(bytes) = std::fs::read(&path) {
            return serde_json::from_slice(&bytes).unwrap_or_default();
        }
        let mut accounts = Self::default();
        if let Some(old) = auth::load(root) {
            accounts.add(Account::from_microsoft(old));
            if accounts.save(root).is_ok() {
                let _ = auth::clear(root);
            }
        }
        accounts
    }

    pub fn save(&self, root: &Root) -> Result<()> {
        let path = store_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::Install(format!("账户序列化失败: {e}")))?;
        crate::paths::write_atomic(&path, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn current(&self) -> Option<&Account> {
        let id = self.current.as_ref()?;
        self.list.iter().find(|a| &a.id == id)
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.list.iter().find(|a| a.id == id)
    }

    /// 加一个账户并换成它。已经有同一个人就更新凭证，保留原来的编号和位置。返回编号。
    pub fn add(&mut self, mut account: Account) -> String {
        let id = if let Some(existing) = self.list.iter_mut().find(|a| a.same_as(&account)) {
            account.id = existing.id.clone();
            if account.skin.is_none() {
                account.skin = existing.skin.clone();
            }
            *existing = account;
            existing.id.clone()
        } else {
            let id = account.id.clone();
            self.list.push(account);
            id
        };
        self.current = Some(id.clone());
        id
    }

    /// 换成某个账户。
    pub fn select(&mut self, id: &str) -> bool {
        if self.get(id).is_some() {
            self.current = Some(id.to_string());
            true
        } else {
            false
        }
    }

    /// 移除一个账户；移除的是正在用的就换到第一个。
    pub fn remove(&mut self, id: &str) -> Option<Account> {
        let index = self.list.iter().position(|a| a.id == id)?;
        let removed = self.list.remove(index);
        if self.current.as_deref() == Some(id) {
            self.current = self.list.first().map(|a| a.id.clone());
        }
        Some(removed)
    }

    /// 改名（只有离线账户能改）。UUID 跟着名字重新算，和游戏的规则一致。
    pub fn rename_offline(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let taken = self
            .list
            .iter()
            .any(|a| a.id != id && a.is_offline() && a.name.eq_ignore_ascii_case(name));
        match self.list.iter_mut().find(|a| a.id == id) {
            Some(account) if account.is_offline() && !taken => {
                account.name = name.to_string();
                account.uuid = crate::launch::offline_uuid(name);
                true
            }
            _ => false,
        }
    }
}

/// 启动游戏要用的身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub uuid: String,
    pub access_token: Option<String>,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
    /// `msa`、`mojang`（外置）或 `legacy`（离线）。
    pub user_type: &'static str,
    /// 外置登录的接口地址和预取的元数据；启动时交给 authlib-injector。
    pub external: Option<External>,
    /// 写进截图的类型名，例如「正版」。
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    pub api: String,
    /// 接口根路径的元数据原文；取不到就是 None，交给 authlib-injector 自己去取。
    pub metadata: Option<String>,
}

/// 准备启动时的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ready {
    Go(Identity),
    /// 凭证失效，要重新登录。带着账户编号和一句原因。
    Relogin {
        id: String,
        reason: String,
    },
}

/// 启动前把账户的凭证弄新：微软到期就刷新；外置先 validate 再 refresh。
/// 刷新后的凭证写回 `accounts.json`。网络不通返回 `Err`，凭证失效返回 `Relogin`。
pub fn ready(root: &Root, id: &str) -> Result<Ready> {
    let mut accounts = Accounts::load(root);
    let Some(account) = accounts.get(id).cloned() else {
        return Err(Error::Login("找不到这个账户".into()));
    };
    let relogin = |accounts: &mut Accounts, reason: &str| -> Result<Ready> {
        if let Some(a) = accounts.list.iter_mut().find(|a| a.id == id) {
            a.stale = true;
        }
        accounts.save(root)?;
        Ok(Ready::Relogin {
            id: id.to_string(),
            reason: reason.to_string(),
        })
    };
    match &account.login {
        Login::Offline => Ok(Ready::Go(Identity {
            name: account.name.clone(),
            uuid: account.uuid.clone(),
            access_token: None,
            xuid: None,
            client_id: None,
            user_type: "legacy",
            external: None,
            kind: account.kind_short(),
        })),
        Login::Microsoft { .. } => {
            let old = account.to_microsoft().expect("是微软账户");
            let fresh = if old.valid_for(300) {
                old
            } else {
                match auth::refresh_account(&old) {
                    Ok(fresh) => fresh,
                    Err(Error::Net { .. }) | Err(Error::Http { .. }) => {
                        return Err(Error::Login("无法连接微软登录服务器".into()));
                    }
                    Err(e) => return relogin(&mut accounts, &e.to_string()),
                }
            };
            let mut updated = Account::from_microsoft(fresh.clone());
            updated.id = account.id.clone();
            if updated != account {
                if let Some(a) = accounts.list.iter_mut().find(|a| a.id == id) {
                    *a = updated;
                }
                accounts.save(root)?;
            }
            Ok(Ready::Go(Identity {
                name: fresh.name,
                uuid: fresh.uuid,
                access_token: Some(fresh.access_token),
                xuid: fresh.xuid,
                client_id: Some(fresh.client_id),
                user_type: "msa",
                external: None,
                kind: account.kind_short(),
            }))
        }
        Login::External {
            api,
            server_name,
            access_token,
            client_token,
            ..
        } => {
            let unreachable = || Error::Login(format!("无法连接 {server_name}"));
            let mut token = access_token.clone();
            let mut name = account.name.clone();
            let valid =
                yggdrasil::validate(api, access_token, client_token).map_err(|_| unreachable())?;
            if !valid {
                let refreshed = yggdrasil::refresh(api, access_token, client_token)
                    .map_err(|_| unreachable())?;
                match refreshed {
                    Some((fresh, profile)) if profile.id == account.uuid => {
                        token = fresh;
                        name = profile.name;
                    }
                    Some(_) => return relogin(&mut accounts, "角色已变更，请重新登录"),
                    None => return relogin(&mut accounts, "登录已失效"),
                }
                if let Some(a) = accounts.list.iter_mut().find(|a| a.id == id) {
                    if let Login::External { access_token, .. } = &mut a.login {
                        *access_token = token.clone();
                    }
                    a.name = name.clone();
                    a.stale = false;
                }
                accounts.save(root)?;
            }
            let metadata = fetch_metadata(api);
            Ok(Ready::Go(Identity {
                name,
                uuid: account.uuid.clone(),
                access_token: Some(token),
                xuid: None,
                client_id: None,
                user_type: "mojang",
                external: Some(External {
                    api: api.clone(),
                    metadata,
                }),
                kind: account.kind_short(),
            }))
        }
    }
}

/// 接口根路径的元数据，启动时预取给 authlib-injector，省得它自己再连一次。
fn fetch_metadata(api: &str) -> Option<String> {
    let http = crate::http::Http::new("official").ok()?;
    let text = http.get_text(api).ok()?;
    yggdrasil::parse_server(api, &text).ok().map(|s| s.metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempRoot(PathBuf, Root);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cairn-accounts-{tag}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let root = Root::new(&dir);
            Self(dir, root)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn microsoft(name: &str, uuid: &str) -> auth::Account {
        auth::Account {
            name: name.into(),
            uuid: uuid.into(),
            access_token: "token".into(),
            refresh_token: "refresh".into(),
            expires_at: u64::MAX / 2,
            xuid: None,
            client_id: "client".into(),
            skin: None,
        }
    }

    #[test]
    fn migrates_the_old_single_account() {
        let temp = TempRoot::new("migrate");
        auth::save(&temp.1, &microsoft("Steve", "abc")).unwrap();
        let accounts = Accounts::load(&temp.1);
        assert_eq!(accounts.list.len(), 1);
        assert_eq!(accounts.current().unwrap().name, "Steve");
        assert!(auth::load(&temp.1).is_none(), "旧文件搬完要删掉");
        assert_eq!(Accounts::load(&temp.1), accounts);
    }

    #[test]
    fn adding_the_same_person_updates_in_place() {
        let mut accounts = Accounts::default();
        let steve = accounts.add(Account::from_microsoft(microsoft("Steve", "a")));
        let alex = accounts.add(Account::from_microsoft(microsoft("Alex", "b")));
        assert_eq!(accounts.current.as_deref(), Some(alex.as_str()));
        let mut renamed = microsoft("Steve2", "a");
        renamed.access_token = "new".into();
        let again = accounts.add(Account::from_microsoft(renamed));
        assert_eq!(again, steve, "同一个 UUID 保留原来的编号");
        assert_eq!(accounts.list.len(), 2);
        assert_eq!(accounts.list[0].name, "Steve2");
        assert_eq!(accounts.current.as_deref(), Some(steve.as_str()));
        let offline = accounts.add(Account::offline("dk"));
        assert_eq!(
            accounts.add(Account::offline("DK")),
            offline,
            "离线名字不分大小写"
        );
        assert_eq!(accounts.list.len(), 3);
    }

    #[test]
    fn removing_the_current_account_picks_another() {
        let mut accounts = Accounts::default();
        let a = accounts.add(Account::offline("a"));
        let b = accounts.add(Account::offline("b"));
        assert!(accounts.select(&a));
        accounts.remove(&a);
        assert_eq!(accounts.current.as_deref(), Some(b.as_str()));
        accounts.remove(&b);
        assert_eq!(accounts.current, None);
    }

    #[test]
    fn offline_accounts_can_be_renamed() {
        let mut accounts = Accounts::default();
        let a = accounts.add(Account::offline("Player"));
        accounts.add(Account::offline("Other"));
        assert!(
            !accounts.rename_offline(&a, "other"),
            "不能和别的离线账户重名"
        );
        assert!(accounts.rename_offline(&a, "Notch"));
        let renamed = accounts.get(&a).unwrap();
        assert_eq!(renamed.uuid, "b50ad385829d3141a2167e7d7539ba7f");
    }

    #[test]
    fn offline_accounts_launch_without_network() {
        let temp = TempRoot::new("ready");
        let mut accounts = Accounts::default();
        let id = accounts.add(Account::offline("Player"));
        accounts.save(&temp.1).unwrap();
        match ready(&temp.1, &id).unwrap() {
            Ready::Go(identity) => {
                assert_eq!(identity.name, "Player");
                assert_eq!(identity.user_type, "legacy");
                assert!(identity.access_token.is_none());
            }
            other => panic!("{other:?}"),
        }
        assert!(ready(&temp.1, "missing").is_err());
    }

    #[test]
    fn store_never_contains_a_password_field() {
        let server = yggdrasil::parse_server(
            "https://skin.example.com/api/yggdrasil",
            r#"{"meta":{"serverName":"Example"}}"#,
        )
        .unwrap();
        let profile = yggdrasil::Profile {
            id: "abc".into(),
            name: "Builder".into(),
        };
        let account =
            Account::external(&server, "me@example.com", "t".into(), "c".into(), &profile);
        let text = serde_json::to_string(&account).unwrap();
        assert!(!text.contains("password"));
        assert_eq!(account.kind_label(), "Example · 外置登录");
    }
}
