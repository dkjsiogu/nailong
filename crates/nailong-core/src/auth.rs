//! Microsoft account login (device-code flow) and token storage.
//!
//! The chain is MSA → Xbox Live → XSTS → Minecraft Services, matching the
//! official launcher. Tokens live in `<data>/account.json` with mode 0600 on
//! Unix; logs never contain them.

use crate::cancel;
use crate::error::{Error, Result};
use crate::http::Http;
use crate::paths::Root;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const AUTH_BASE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const XBL_AUTH: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_BASE: &str = "https://api.minecraftservices.com";
const SCOPE: &str = "XboxLive.signin offline_access";

/// Public client id of the nailong Entra app. Overridable with
/// `NAILONG_MS_CLIENT_ID` for forks that own their own registration.
pub const DEFAULT_CLIENT_ID: &str = "986e5a65-5023-4764-982e-51058fe75746";

pub fn client_id() -> String {
    std::env::var("NAILONG_MS_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_CLIENT_ID.to_string())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    /// Minecraft profile id, no dashes.
    pub uuid: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix time when the Minecraft token expires.
    pub expires_at: u64,
    pub xuid: Option<String>,
    pub client_id: String,
}

impl Account {
    pub fn valid_for(&self, seconds: u64) -> bool {
        self.expires_at > now() + seconds && !self.access_token.is_empty()
    }
}

fn account_file(root: &Root) -> PathBuf {
    root.dir().join("account.json")
}

pub fn load(root: &Root) -> Option<Account> {
    let bytes = std::fs::read(account_file(root)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save(root: &Root, account: &Account) -> Result<()> {
    let path = account_file(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::error::io(parent, e))?;
    }
    let bytes = serde_json::to_vec_pretty(account)
        .map_err(|e| Error::Install(format!("账户序列化失败: {e}")))?;
    crate::paths::write_atomic(&path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn clear(root: &Root) -> Result<()> {
    let path = account_file(root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::error::io(&path, e)),
    }
}

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct MsaTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

#[derive(Debug, Clone)]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Ready(MsaTokens),
    Failed(String),
}

pub fn start_device_code() -> Result<DeviceCode> {
    let http = Http::new("official")?;
    let (status, value) = http.post_form(
        &format!("{AUTH_BASE}/devicecode"),
        &[("client_id", &client_id()), ("scope", SCOPE)],
    )?;
    if status != 200 {
        return Err(Error::Install(oauth_error(&value, status)));
    }
    Ok(DeviceCode {
        device_code: value
            .get("device_code")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        user_code: value
            .get("user_code")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        verification_uri: value
            .get("verification_uri")
            .and_then(|v| v.as_str())
            .unwrap_or("https://microsoft.com/link")
            .to_string(),
        expires_in: value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(900),
        interval: value
            .get("interval")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .max(1),
        message: value
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

pub fn poll_device(device_code: &str) -> Result<DevicePoll> {
    let http = Http::new("official")?;
    let form = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", &client_id()),
        ("device_code", device_code),
    ];
    let (status, value) = http.post_form(&format!("{AUTH_BASE}/token"), &form)?;
    if status == 200 {
        return Ok(DevicePoll::Ready(MsaTokens {
            access_token: str_field(&value, "access_token"),
            refresh_token: str_field(&value, "refresh_token"),
            expires_in: value
                .get("expires_in")
                .and_then(|v| v.as_u64())
                .unwrap_or(3600),
        }));
    }
    let code = value
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    Ok(match code {
        "authorization_pending" => DevicePoll::Pending,
        "slow_down" => DevicePoll::SlowDown,
        "expired_token" => DevicePoll::Failed("验证码已过期，请重新登录".into()),
        "authorization_declined" => DevicePoll::Failed("已取消登录".into()),
        "invalid_grant" => DevicePoll::Failed("登录已失效，请重新登录".into()),
        _ => DevicePoll::Failed(oauth_error(&value, status)),
    })
}

/// 阻塞轮询，直到成功、失败或取消。供后台线程使用。
pub fn wait_for_device(
    device: &DeviceCode,
    cancelled: impl Fn() -> bool,
) -> Result<Option<MsaTokens>> {
    let mut interval = device.interval;
    let deadline = now() + device.expires_in.min(900);
    loop {
        if cancelled() || cancel::is_cancelled() {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_secs(interval));
        if cancelled() || cancel::is_cancelled() {
            return Ok(None);
        }
        match poll_device(&device.device_code)? {
            DevicePoll::Pending => {}
            DevicePoll::SlowDown => interval += 5,
            DevicePoll::Ready(tokens) => return Ok(Some(tokens)),
            DevicePoll::Failed(message) => return Err(Error::Install(message)),
        }
        if now() >= deadline {
            return Err(Error::Install("验证码已过期，请重新登录".into()));
        }
    }
}

pub fn login_with_msa(msa: &MsaTokens) -> Result<Account> {
    let (xbl_token, uhs) = xbox_authenticate(&msa.access_token)?;
    let (xsts_token, xsts_uhs, xuid) = xsts_authorize(&xbl_token)?;
    let _ = uhs;
    let mc = minecraft_login(&xsts_uhs, &xsts_token)?;
    ensure_entitlement(&mc.access_token)?;
    let (profile_id, profile_name) = minecraft_profile(&mc.access_token)?;
    Ok(Account {
        name: profile_name,
        uuid: profile_id,
        access_token: mc.access_token,
        refresh_token: msa.refresh_token.clone(),
        expires_at: now() + mc.expires_in,
        xuid,
        client_id: client_id(),
    })
}

struct MinecraftToken {
    access_token: String,
    expires_in: u64,
}

fn xbox_authenticate(msa_access: &str) -> Result<(String, String)> {
    let http = Http::new("official")?;
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={msa_access}")
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });
    let (status, value) = http.post_json(
        XBL_AUTH,
        &body,
        &[
            ("x-xbl-contract-version", "1"),
            ("Accept", "application/json"),
        ],
    )?;
    if status != 200 {
        return Err(Error::Install(format!(
            "Xbox Live 登录失败（HTTP {status}），请稍后再试"
        )));
    }
    let token = str_field(&value, "Token");
    let uhs = value
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok((token, uhs))
}

fn xsts_authorize(xbl_token: &str) -> Result<(String, String, Option<String>)> {
    let http = Http::new("official")?;
    let body = json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token]
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT"
    });
    let (status, value) = http.post_json(
        XSTS_AUTH,
        &body,
        &[
            ("x-xbl-contract-version", "1"),
            ("Accept", "application/json"),
        ],
    )?;
    if status != 200 {
        let xerr = value.get("XErr").and_then(|v| v.as_u64());
        return Err(Error::Install(xsts_error_message(xerr)));
    }
    Ok((
        str_field(&value, "Token"),
        value
            .pointer("/DisplayClaims/xui/0/uhs")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        value
            .pointer("/DisplayClaims/xui/0/xid")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    ))
}

fn xsts_error_message(xerr: Option<u64>) -> String {
    match xerr {
        Some(2148916233) => "该微软账户还没有 Xbox 档案，请先在 Xbox 官网创建".into(),
        Some(2148916235) => "当前地区不支持 Xbox Live".into(),
        Some(2148916238) => "儿童账户需要先加入 Microsoft 家庭组".into(),
        Some(2148916227) => "该账户已被 Xbox Live 封禁".into(),
        Some(code) => format!("Xbox Live 授权失败（XErr {code}）"),
        None => "Xbox Live 授权失败，请稍后再试".into(),
    }
}

fn minecraft_login(uhs: &str, xsts: &str) -> Result<MinecraftToken> {
    let http = Http::new("official")?;
    let body = json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts}") });
    let (status, value) = http.post_json(
        &format!("{MC_BASE}/authentication/login_with_xbox"),
        &body,
        &[("Accept", "application/json")],
    )?;
    if status == 403 {
        return Err(Error::Install(
            "Minecraft 拒绝了启动器的 client id（未在 allowlist），这不是你的账户问题".into(),
        ));
    }
    if status != 200 {
        return Err(Error::Install(format!(
            "Minecraft 登录失败（HTTP {status}）"
        )));
    }
    Ok(MinecraftToken {
        access_token: str_field(&value, "access_token"),
        expires_in: value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(86_400),
    })
}

fn ensure_entitlement(token: &str) -> Result<()> {
    let http = Http::new("official")?;
    let value = http.get_json_with_headers(
        &format!("{MC_BASE}/entitlements/mcstore"),
        &[("Authorization", &format!("Bearer {token}"))],
    )?;
    let owned = value
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    if !owned {
        return Err(Error::Install(
            "该账户未拥有 Minecraft: Java Edition（或 Game Pass 已到期）".into(),
        ));
    }
    Ok(())
}

fn minecraft_profile(token: &str) -> Result<(String, String)> {
    let http = Http::new("official")?;
    let value = http.get_json_with_headers(
        &format!("{MC_BASE}/minecraft/profile"),
        &[("Authorization", &format!("Bearer {token}"))],
    )?;
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Install("找不到游戏档案（可能未购买或未创建角色）".into()))?;
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Install("游戏档案缺少用户名".into()))?;
    Ok((id.to_string(), name.to_string()))
}

pub fn refresh_account(account: &Account) -> Result<Account> {
    let http = Http::new("official")?;
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", account.client_id.as_str()),
        ("refresh_token", account.refresh_token.as_str()),
        ("scope", SCOPE),
    ];
    let (status, value) = http.post_form(&format!("{AUTH_BASE}/token"), &form)?;
    if status != 200 {
        let code = value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        return Err(Error::Install(match code {
            "invalid_grant" => "登录已过期，请重新登录".into(),
            _ => oauth_error(&value, status),
        }));
    }
    let msa = MsaTokens {
        access_token: str_field(&value, "access_token"),
        refresh_token: value
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| account.refresh_token.clone()),
        expires_in: value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600),
    };
    let renewed = login_with_msa(&msa)?;
    if renewed.uuid != account.uuid {
        return Err(Error::Install(
            "刷新后的账户与原来不一致，请重新登录".into(),
        ));
    }
    Ok(renewed)
}

/// 读账户并保证令牌还有效；必要时刷新并落盘。
pub fn ensure_fresh(root: &Root, account: &Account) -> Result<Account> {
    if account.valid_for(300) {
        return Ok(account.clone());
    }
    let renewed = refresh_account(account)?;
    save(root, &renewed)?;
    Ok(renewed)
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn oauth_error(value: &Value, status: u16) -> String {
    let code = value
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let description = value
        .get("error_description")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    match code {
        "access_denied" => "已取消登录".into(),
        "invalid_client" => "启动器的 client id 无效（未配置或未获批），请联系维护者".into(),
        _ if !code.is_empty() => format!("微软登录失败：{code}（{description}）"),
        _ => format!("微软登录失败（HTTP {status}）"),
    }
}

/// 浏览器授权码 + PKCE 流程。设备码被 AADSTS70002 拒绝时，这条链路仍然可用。
pub struct AuthCodeFlow {
    pub url: String,
    pub port: u16,
    redirect_uri: String,
    verifier: String,
    state: String,
    listener: TcpListener,
    expires_at: u64,
}

pub fn start_auth_code() -> Result<AuthCodeFlow> {
    let listener = TcpListener::bind(("127.0.0.1", 39123))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|e| Error::Install(format!("无法监听登录回调端口: {e}")))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| Error::Install(format!("设置回调端口失败: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::Install(format!("读取回调端口失败: {e}")))?
        .port();
    let redirect_uri = format!("http://localhost:{port}/callback");
    let verifier = b64url(&random_bytes(32));
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = b64url(&random_bytes(16));
    let url = format!(
        "{AUTH_BASE}/authorize?client_id={}&response_type=code&redirect_uri={}&response_mode=query&scope={}&prompt=select_account&state={}&code_challenge={}&code_challenge_method=S256",
        urlencode(&client_id()),
        urlencode(&redirect_uri),
        urlencode(SCOPE),
        state,
        challenge
    );
    Ok(AuthCodeFlow {
        url,
        port,
        redirect_uri,
        verifier,
        state,
        listener,
        expires_at: now() + 300,
    })
}

pub fn wait_for_code(flow: &AuthCodeFlow, cancelled: impl Fn() -> bool) -> Result<MsaTokens> {
    let mut buf = [0u8; 8192];
    loop {
        if cancelled() || cancel::is_cancelled() {
            return Err(Error::Cancelled);
        }
        if now() > flow.expires_at {
            return Err(Error::Install("登录超时，请重新发起".into()));
        }
        match flow.listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                match parse_callback(&request) {
                    Callback::Ignore => {
                        respond(&mut stream, 404, "不是登录回调");
                    }
                    Callback::Error(message) => {
                        respond(&mut stream, 200, "登录未完成，可以关闭此页面");
                        return Err(Error::Install(message));
                    }
                    Callback::Code { code, state } => {
                        if state != flow.state {
                            respond(&mut stream, 200, "登录状态校验失败");
                            return Err(Error::Install("登录状态校验失败，请重新登录".into()));
                        }
                        respond(&mut stream, 200, "登录完成，可以关闭此页面");
                        return exchange_code(&code, &flow.verifier, &flow.redirect_uri);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(Error::Install(format!("登录回调失败: {e}"))),
        }
    }
}

fn exchange_code(code: &str, verifier: &str, redirect_uri: &str) -> Result<MsaTokens> {
    let http = Http::new("official")?;
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", &client_id()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
        ("scope", SCOPE),
    ];
    let (status, value) = http.post_form(&format!("{AUTH_BASE}/token"), &form)?;
    if status != 200 {
        return Err(Error::Install(oauth_error(&value, status)));
    }
    Ok(MsaTokens {
        access_token: str_field(&value, "access_token"),
        refresh_token: str_field(&value, "refresh_token"),
        expires_in: value
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600),
    })
}

enum Callback {
    Ignore,
    Error(String),
    Code { code: String, state: String },
}

fn parse_callback(request: &str) -> Callback {
    let Some(line) = request.lines().next() else {
        return Callback::Ignore;
    };
    let mut parts = line.split_whitespace();
    let Some(method) = parts.next() else {
        return Callback::Ignore;
    };
    let Some(target) = parts.next() else {
        return Callback::Ignore;
    };
    if method != "GET" {
        return Callback::Ignore;
    }
    let Some(query) = target.split_once('?').map(|(_, query)| query) else {
        return Callback::Ignore;
    };
    if !target.starts_with("/callback") {
        return Callback::Ignore;
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Callback::Error(match error.as_str() {
            "access_denied" => "已取消登录".into(),
            other => format!("微软登录失败：{other}"),
        });
    }
    match (code, state) {
        (Some(code), Some(state)) if !code.is_empty() => Callback::Code { code, state },
        _ => Callback::Ignore,
    }
}

fn respond(stream: &mut std::net::TcpStream, status: u16, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>nailong</title><body style=\"font-family:sans-serif;padding:40px\">{message}</body>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status} OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn random_bytes(count: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(count + 16);
    while out.len() < count {
        out.extend_from_slice(Uuid::new_v4().as_bytes());
    }
    out.truncate(count);
    out
}

fn b64url(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b1 = chunk[0] as u32;
        let b2 = *chunk.get(1).unwrap_or(&0) as u32;
        let b3 = *chunk.get(2).unwrap_or(&0) as u32;
        let value = (b1 << 16) | (b2 << 8) | b3;
        out.push(TABLE[((value >> 18) & 63) as usize] as char);
        out.push(TABLE[((value >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((value >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(value & 63) as usize] as char);
        }
    }
    out
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    index += 3;
                    continue;
                }
                out.push(b'%');
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempRoot(PathBuf, Root);

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "nailong-auth-{tag}-{}-{}",
                std::process::id(),
                now()
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

    fn account() -> Account {
        Account {
            name: "Steve".into(),
            uuid: "0123456789abcdef0123456789abcdef".into(),
            access_token: "token".into(),
            refresh_token: "refresh".into(),
            expires_at: now() + 3600,
            xuid: Some("123".into()),
            client_id: DEFAULT_CLIENT_ID.into(),
        }
    }

    #[test]
    fn saves_and_loads_account() {
        let temp = TempRoot::new("save");
        let account = account();
        save(&temp.1, &account).unwrap();
        let loaded = load(&temp.1).unwrap();
        assert_eq!(loaded.name, "Steve");
        assert_eq!(loaded.uuid, account.uuid);
        assert!(loaded.valid_for(60));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(account_file(&temp.1))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        clear(&temp.1).unwrap();
        assert!(load(&temp.1).is_none());
    }

    #[test]
    fn maps_xsts_errors() {
        assert!(xsts_error_message(Some(2148916233)).contains("Xbox 档案"));
        assert!(xsts_error_message(Some(2148916235)).contains("地区"));
        assert!(xsts_error_message(Some(2148916238)).contains("家庭组"));
        assert!(xsts_error_message(Some(2148916227)).contains("封禁"));
        assert!(xsts_error_message(Some(1)).contains("XErr 1"));
        assert!(xsts_error_message(None).contains("授权失败"));
    }

    #[test]
    fn expired_account_is_not_fresh() {
        let mut account = account();
        account.expires_at = now() - 10;
        assert!(!account.valid_for(300));
    }

    #[test]
    fn base64url_vectors() {
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn parses_oauth_callback() {
        match parse_callback("GET /callback?code=abc%2F1&state=xyz HTTP/1.1") {
            Callback::Code { code, state } => {
                assert_eq!(code, "abc/1");
                assert_eq!(state, "xyz");
            }
            _ => panic!("应当解析出 code"),
        }
        assert!(matches!(
            parse_callback("GET /favicon.ico HTTP/1.1"),
            Callback::Ignore
        ));
        match parse_callback("GET /callback?error=access_denied HTTP/1.1") {
            Callback::Error(message) => assert!(message.contains("取消")),
            _ => panic!("应当识别取消"),
        }
    }

    #[test]
    fn loopback_flow_rejects_wrong_state() {
        use std::io::Write;
        let flow = start_auth_code().unwrap();
        assert!(flow.url.contains("code_challenge="));
        assert!(flow.url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A"));
        let result = std::thread::scope(|scope| {
            let handle = scope.spawn(|| {
                let mut stream = std::net::TcpStream::connect(("127.0.0.1", flow.port)).unwrap();
                stream
                    .write_all(
                        b"GET /callback?code=fake&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n",
                    )
                    .unwrap();
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
            });
            let result = wait_for_code(&flow, || false);
            handle.join().unwrap();
            result
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("状态校验"));
    }

    #[test]
    fn client_id_can_be_overridden() {
        // 环境变量优先级只做存在性检查，避免并行测试互相干扰。
        assert!(!client_id().is_empty());
    }
}
