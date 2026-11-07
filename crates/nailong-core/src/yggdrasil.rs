//! 外置登录：LittleSkin 这类皮肤站，走 authlib-injector 的 Yggdrasil 协议。
//!
//! 规范：<https://github.com/yushijinhun/authlib-injector/wiki>。要点：
//!
//! - 玩家只要输入 `littleskin.cn`：补上 `https://`，看响应头 `X-Authlib-Injector-API-Location`
//!   找到真正的接口地址（ALI）；不降级到 http，http 的服务器要明显提示。
//! - 密码只用来换登录凭证（accessToken），任何时候都不保存密码。
//! - 一个账号有多个角色时让玩家选，用 refresh 带上 `selectedProfile` 绑定。
//! - 启动前依次 validate → refresh；都不行才让玩家重新输入密码。
//! - 启动时加 `-javaagent:authlib-injector.jar=<API>` 和预取的元数据（Base64）。

use crate::error::{Error, Result};
use crate::http::Http;
use crate::paths::Root;
use serde_json::{Value, json};
use std::path::PathBuf;

/// 一个认证服务器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// 接口地址，以 `/` 结尾，例如 `https://littleskin.cn/api/yggdrasil/`。
    pub api: String,
    /// 服务器自己报的名字；没报就用主机名。
    pub name: String,
    pub homepage: Option<String>,
    pub register: Option<String>,
    /// 登录名不是邮箱（界面上写「账号」而不是「邮箱」）。
    pub non_email_login: bool,
    /// 没加密的 http 服务器：密码会明文传输，要提醒。
    pub insecure: bool,
    /// 接口根路径返回的元数据原文，启动时 Base64 后交给 authlib-injector。
    pub metadata: String,
}

/// 一个游戏角色。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// 无短横线的 UUID。
    pub id: String,
    pub name: String,
}

/// 用密码登录的结果。
#[derive(Debug, Clone)]
pub struct Login {
    pub access_token: String,
    pub client_token: String,
    pub profiles: Vec<Profile>,
    pub selected: Option<Profile>,
}

/// 补全并解析玩家输入的服务器地址，取回元数据。
pub fn resolve(input: &str) -> Result<Server> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::Login("请输入认证服务器地址".into()));
    }
    let start = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    if !start.starts_with("https://") && !start.starts_with("http://") {
        return Err(Error::Login("地址要以 https:// 开头".into()));
    }
    let http = Http::new("official")?;
    let (status, location, body) =
        http.get_with_header(&start, "X-Authlib-Injector-API-Location")?;
    let mut api = start.clone();
    let mut body = Some((status, body));
    if let Some(location) = location {
        let target = join_url(&start, &location);
        // 不降级：https 指向 http 的不跟。
        if !(start.starts_with("https://") && target.starts_with("http://"))
            && trim_slash(&target) != trim_slash(&start)
        {
            api = target;
            body = None;
        }
    }
    let (status, text) = match body {
        Some(found) => found,
        None => {
            let (status, _, text) =
                http.get_with_header(&api, "X-Authlib-Injector-API-Location")?;
            (status, text)
        }
    };
    if status != 200 {
        return Err(Error::Login(format!("不是认证服务器（HTTP {status}）")));
    }
    parse_server(&api, &text)
}

/// 从接口根路径的响应读出服务器信息。
pub fn parse_server(api: &str, text: &str) -> Result<Server> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| Error::Login("不是认证服务器".into()))?;
    if value.get("meta").is_none() && value.get("signaturePublickey").is_none() {
        return Err(Error::Login("不是认证服务器".into()));
    }
    let meta = value.get("meta").cloned().unwrap_or(Value::Null);
    let api = format!("{}/", trim_slash(api));
    let text_of = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_string);
    let host = api
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(&api)
        .to_string();
    Ok(Server {
        name: text_of(meta.get("serverName"))
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(host),
        homepage: text_of(meta.pointer("/links/homepage")),
        register: text_of(meta.pointer("/links/register")),
        non_email_login: meta
            .get("feature.non_email_login")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        insecure: api.starts_with("http://"),
        metadata: text.to_string(),
        api,
    })
}

/// 用账号和密码登录。密码只在这一次请求里用。
pub fn authenticate(api: &str, login: &str, password: &str) -> Result<Login> {
    let client_token = uuid::Uuid::new_v4().simple().to_string();
    let body = json!({
        "username": login,
        "password": password,
        "clientToken": client_token,
        "requestUser": true,
        "agent": { "name": "Minecraft", "version": 1 },
    });
    let value = post(api, "authserver/authenticate", &body)?;
    let profiles = value
        .get("availableProfiles")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(profile_of).collect())
        .unwrap_or_default();
    Ok(Login {
        access_token: str_of(&value, "accessToken")?,
        client_token: value
            .get("clientToken")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(client_token),
        profiles,
        selected: value.get("selectedProfile").and_then(profile_of),
    })
}

/// 给还没绑定角色的凭证选一个角色，返回新的 accessToken。
pub fn select_profile(
    api: &str,
    access_token: &str,
    client_token: &str,
    profile: &Profile,
) -> Result<String> {
    let body = json!({
        "accessToken": access_token,
        "clientToken": client_token,
        "requestUser": true,
        "selectedProfile": { "id": profile.id, "name": profile.name },
    });
    let value = post(api, "authserver/refresh", &body)?;
    str_of(&value, "accessToken")
}

/// 凭证是否还能用。网络不通是 `Err`，服务器说不行是 `Ok(false)`。
pub fn validate(api: &str, access_token: &str, client_token: &str) -> Result<bool> {
    let http = Http::new("official")?;
    let (status, _) = http.post_json(
        &endpoint(api, "authserver/validate"),
        &json!({ "accessToken": access_token, "clientToken": client_token }),
        &[],
    )?;
    Ok(status == 204 || status == 200)
}

/// 换一张新凭证。`Ok(None)` 表示旧凭证已经彻底失效，要重新输入密码。
pub fn refresh(
    api: &str,
    access_token: &str,
    client_token: &str,
) -> Result<Option<(String, Profile)>> {
    let http = Http::new("official")?;
    let (status, value) = http.post_json(
        &endpoint(api, "authserver/refresh"),
        &json!({ "accessToken": access_token, "clientToken": client_token, "requestUser": true }),
        &[],
    )?;
    if status != 200 {
        return Ok(None);
    }
    let token = str_of(&value, "accessToken")?;
    let profile = value
        .get("selectedProfile")
        .and_then(profile_of)
        .ok_or_else(|| Error::Login("服务器没有返回角色".into()))?;
    Ok(Some((token, profile)))
}

/// 角色的皮肤图片地址（没有就是 None）。
pub fn skin_url(api: &str, uuid: &str) -> Option<String> {
    let http = Http::new("official").ok()?;
    let value = http
        .get_json(&endpoint(
            api,
            &format!("sessionserver/session/minecraft/profile/{uuid}?unsigned=true"),
        ))
        .ok()?;
    textures_skin(&value)
}

/// 档案里 `textures` 属性（Base64 的 JSON）里的皮肤地址。
pub fn textures_skin(profile: &Value) -> Option<String> {
    let encoded = profile
        .get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(Value::as_str) == Some("textures"))?
        .get("value")?
        .as_str()?;
    let decoded = b64_decode(encoded)?;
    let textures: Value = serde_json::from_slice(&decoded).ok()?;
    textures
        .pointer("/textures/SKIN/url")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 皮肤站网页上「拖到启动器」的数据：`authlib-injector:yggdrasil-server:<编码后的 API 地址>`。
pub fn parse_drop(text: &str) -> Option<String> {
    let rest = text
        .trim()
        .strip_prefix("authlib-injector:yggdrasil-server:")?;
    let api = crate::auth::percent_decode(rest);
    (api.starts_with("https://") || api.starts_with("http://")).then_some(api)
}

/// 启动时交给 authlib-injector 的预取元数据。
pub fn prefetched(metadata: &str) -> String {
    b64_encode(metadata.as_bytes())
}

/// 准备好 authlib-injector：本地有就用，顺便看看有没有新版；没有就下载并按 SHA-256 校验。
/// 官方连不上自动走 BMCLAPI 的镜像。
pub fn injector_jar(root: &Root) -> Result<PathBuf> {
    let dir = root.dir().join("authlib-injector");
    let local = newest_local(&dir);
    let http = Http::new("official")?;
    let latest = match http.get_json("https://authlib-injector.yushi.moe/artifact/latest.json") {
        Ok(value) => value,
        Err(e) => return local.ok_or(e),
    };
    let version = latest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("latest");
    let name = format!("authlib-injector-{version}.jar");
    let path = dir.join(&name);
    if path.is_file() {
        return Ok(path);
    }
    let (Some(url), Some(sha256)) = (
        latest.get("download_url").and_then(Value::as_str),
        latest.pointer("/checksums/sha256").and_then(Value::as_str),
    ) else {
        return local.ok_or_else(|| Error::Install("authlib-injector 的版本信息不完整".into()));
    };
    let bytes = match http.get_bytes(url) {
        Ok(bytes) => bytes,
        Err(e) => return local.ok_or(e),
    };
    let actual = {
        use sha2::{Digest, Sha256};
        crate::http::hex(&Sha256::digest(&bytes))
    };
    if !actual.eq_ignore_ascii_case(sha256) {
        return local.ok_or_else(|| Error::Checksum {
            path: path.clone(),
            expected: sha256.to_string(),
            actual,
        });
    }
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::io(&dir, e))?;
    crate::paths::write_atomic(&path, &bytes)?;
    Ok(path)
}

fn newest_local(dir: &std::path::Path) -> Option<PathBuf> {
    let mut jars: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "jar")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("authlib-injector-"))
        })
        .collect();
    jars.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    jars.pop()
}

fn post(api: &str, path: &str, body: &Value) -> Result<Value> {
    let http = Http::new("official")?;
    let (status, value) = http.post_json(&endpoint(api, path), body, &[])?;
    if (200..300).contains(&status) {
        return Ok(value);
    }
    Err(Error::Login(friendly_error(status, &value)))
}

/// 服务器的英文错误换成能照着做的话。
pub fn friendly_error(status: u16, value: &Value) -> String {
    let message = value
        .get("errorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lower = message.to_ascii_lowercase();
    if lower.contains("invalid credentials")
        || lower.contains("username or password")
        || message.contains("用户名或密码")
        || message.contains("密码错误")
    {
        "账号或密码不对".into()
    } else if lower.contains("invalid token") {
        "登录已失效".into()
    } else if lower.contains("already has a profile") {
        "请重新登录".into()
    } else if status == 429 || lower.contains("too many") {
        "请求过于频繁，请稍后再试".into()
    } else if !message.is_empty() {
        format!("服务器返回：{message}")
    } else {
        format!("服务器返回 HTTP {status}")
    }
}

fn endpoint(api: &str, path: &str) -> String {
    format!("{}/{path}", trim_slash(api))
}

fn trim_slash(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// 把 ALI 头（可能是相对地址）接到请求地址上。
fn join_url(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if let Some(path) = location.strip_prefix('/') {
        return format!("{scheme}://{host}/{path}");
    }
    let dir = match rest.rfind('/') {
        Some(i) if i >= host.len() => rest[..=i].to_string(),
        _ => format!("{host}/"),
    };
    format!("{scheme}://{dir}{location}")
}

fn profile_of(value: &Value) -> Option<Profile> {
    Some(Profile {
        id: value.get("id")?.as_str()?.replace('-', ""),
        name: value.get("name")?.as_str()?.to_string(),
    })
}

fn str_of(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Login(format!("服务器的回答里缺少 {key}")))
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = buf << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for text in ["", "f", "fo", "foo", "foob", "fooba", "foobar", "皮肤站"] {
            let encoded = b64_encode(text.as_bytes());
            assert_eq!(b64_decode(&encoded).unwrap(), text.as_bytes());
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
    }

    #[test]
    fn resolves_relative_api_locations() {
        assert_eq!(
            join_url("https://skin.example.com/", "/api/yggdrasil/"),
            "https://skin.example.com/api/yggdrasil/"
        );
        assert_eq!(
            join_url("https://skin.example.com", "/api/yggdrasil"),
            "https://skin.example.com/api/yggdrasil"
        );
        assert_eq!(
            join_url("https://a.example.com/x/y", "api/"),
            "https://a.example.com/x/api/"
        );
        assert_eq!(
            join_url("https://a.example.com/", "https://b.example.com/api"),
            "https://b.example.com/api"
        );
    }

    #[test]
    fn reads_server_metadata() {
        let text = r#"{"meta":{"serverName":"LittleSkin","links":{"homepage":"https://littleskin.cn","register":"https://littleskin.cn/auth/register"},"feature.non_email_login":true},"skinDomains":["littleskin.cn"],"signaturePublickey":"-----BEGIN PUBLIC KEY-----"}"#;
        let server = parse_server("https://littleskin.cn/api/yggdrasil", text).unwrap();
        assert_eq!(server.api, "https://littleskin.cn/api/yggdrasil/");
        assert_eq!(server.name, "LittleSkin");
        assert_eq!(
            server.register.as_deref(),
            Some("https://littleskin.cn/auth/register")
        );
        assert!(server.non_email_login);
        assert!(!server.insecure);
        assert!(parse_server("https://example.com/", "<html>").is_err());
        assert!(parse_server("https://example.com/", "{}").is_err());
        let nameless = parse_server("http://127.0.0.1:8080/api", r#"{"meta":{}}"#).unwrap();
        assert_eq!(nameless.name, "127.0.0.1:8080");
        assert!(nameless.insecure);
    }

    #[test]
    fn reads_dropped_server_links() {
        assert_eq!(
            parse_drop(
                "authlib-injector:yggdrasil-server:https%3A%2F%2Flittleskin.cn%2Fapi%2Fyggdrasil"
            )
            .as_deref(),
            Some("https://littleskin.cn/api/yggdrasil")
        );
        assert_eq!(parse_drop("https://littleskin.cn"), None);
        assert_eq!(
            parse_drop("authlib-injector:yggdrasil-server:javascript%3Aalert(1)"),
            None
        );
    }

    #[test]
    fn reads_skin_from_textures_property() {
        let textures = r#"{"textures":{"SKIN":{"url":"https://littleskin.cn/textures/abc"}}}"#;
        let profile = json!({"id":"x","name":"y","properties":[{"name":"textures","value": b64_encode(textures.as_bytes())}]});
        assert_eq!(
            textures_skin(&profile).as_deref(),
            Some("https://littleskin.cn/textures/abc")
        );
        assert_eq!(textures_skin(&json!({"properties":[]})), None);
    }

    #[test]
    #[ignore = "连外网"]
    fn live_resolves_littleskin_and_downloads_the_injector() {
        let server = resolve("littleskin.cn").unwrap();
        assert_eq!(server.api, "https://littleskin.cn/api/yggdrasil/");
        assert_eq!(server.name, "LittleSkin");
        assert!(server.non_email_login);
        let wrong =
            authenticate(&server.api, "nobody@example.invalid", "wrong-password").unwrap_err();
        assert!(wrong.to_string().contains("账号或密码不对"), "{wrong}");
        let dir =
            std::env::temp_dir().join(format!("cairn-injector-{}", uuid::Uuid::new_v4().simple()));
        let root = Root::new(&dir);
        let jar = injector_jar(&root).unwrap();
        assert!(jar.is_file());
        assert_eq!(injector_jar(&root).unwrap(), jar, "第二次直接用本地的");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn explains_server_errors() {
        let wrong = json!({"error":"ForbiddenOperationException","errorMessage":"Invalid credentials. Invalid username or password."});
        assert_eq!(friendly_error(403, &wrong), "账号或密码不对");
        let littleskin = json!({"error":"ForbiddenOperationException","errorMessage":"用户名或密码错误。Incorrect username or password."});
        assert_eq!(friendly_error(403, &littleskin), "账号或密码不对");
        let token = json!({"error":"ForbiddenOperationException","errorMessage":"Invalid token."});
        assert_eq!(friendly_error(403, &token), "登录已失效");
        assert_eq!(friendly_error(502, &Value::Null), "服务器返回 HTTP 502");
    }
}
