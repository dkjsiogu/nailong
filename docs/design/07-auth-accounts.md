# 07 · 账户与认证

> 目标：微软正版登录体验接近官方启动器；离线账户可用；令牌安全存储、静默刷新、失败可解释。
> **当前决策（2026-10-20）**：微软登录不作为 v1.0 的阻塞项。v1.0 先交付离线账户 + 账户抽象；client id 与 Minecraft Services allowlist 就绪后，再把正版登录作为 v1.1 能力启用。设计保留，代码可用 feature/配置开关隐藏。
> 安全总览见 [09](09-security-legal.md)。

## 1. 账户模型

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,              // 本地 UUID，稳定
    pub kind: AccountKind,          // Microsoft | Offline | Authlib
    pub username: String,
    pub uuid: Uuid,
    pub skin_model: SkinModel,      // Classic | Slim
    pub authlib_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Secret {
    Microsoft {
        refresh_token: String,      // 长期；轮换后更新
        msa_access_token: String,
        msa_expires_at: DateTime<Utc>,
        mc_access_token: String,
        mc_expires_at: DateTime<Utc>,
        xuid: Option<String>,
        client_id: String,
    },
    Offline,
    Authlib {
        access_token: String,
        client_token: String,
        expires_at: Option<DateTime<Utc>>,
    },
}
```

- `accounts.json` 只存**公开元数据**（名字、UUID、皮肤模型、服务器地址）；密钥单独存系统钥匙串。
- 每个账户一个钥匙串条目：service = `nailong`，account = `account/<id>`，secret = JSON。
- 启动时只加载元数据；令牌按需读取（避免每次启动解锁钥匙串）。

## 2. 微软登录

### 2.1 应用注册（准备上线正版登录时）

> **⚠️ 2026 年的硬门槛：Minecraft Services 对 client id 做 allowlist。**
> 新注册的 Azure 应用即使 OAuth、Xbox Live、XSTS 全部成功，调用
> `POST https://api.minecraftservices.com/authentication/login_with_xbox` 仍会返回
> **HTTP 403 `Invalid app registration`**。这不是 Entra 配置问题，无法通过权限/重定向 URI 解决；
> 需要走 Mojang/Xbox 的 AppID 审批（Minecraft 官方 AppID registration / Xbox Developer 流程），
> 审批不保证通过，且耗时不可控。**不能**复用 PCL/Prism/官方启动器的 client id。
> 参考：[Microsoft Q&A 讨论](https://learn.microsoft.com/en-us/answers/questions/5984225/minecraft-services-http-403-invalid-app-registrati)。
>
> 项目动作：
> 1. **不阻塞 v1.0**：v1.0 先交付离线账户；正版登录标记为 v1.1/实验性。
> 2. 计划上线正版登录前预留审批时间（周期不可控），再提交 client id 审批。
> 3. 在获批前，微软登录入口可隐藏或显示“暂不可用”，默认引导离线账户；开发测试用已获授权的测试 client id（仅本地）。
> 4. UI 对 403 显示明确原因与申请指引，不显示“登录失败”这类模糊文案。
> 5. 发布前确认 client id 已在 allowlist；否则不要宣称支持正版登录。

1. Azure Portal → App registrations → New registration。
2. Supported account types：**Personal Microsoft accounts**（`consumers`）；如需组织账户可加 `common`，但 Minecraft 只需要个人账户。
3. Redirect URI：平台选 **Mobile and desktop applications**，填 `http://localhost`（回环地址；动态端口）。
   若租户策略不允许通配端口，则注册固定端口，如 `http://localhost:39123`。
4. Authentication → **Allow public client flows = Yes**（设备码需要）。
5. 不需要 client secret；client id 是公开信息，可写入配置/编译期常量。
6. API 权限：不需要 Graph 权限；使用 `XboxLive.signin` scope。
7. **向 Minecraft/Mojang 提交 AppID/allowlist 审批**（见上方警告）；获批后 `login_with_xbox` 才可用。
8. 把 client id 通过构建期环境变量（如 `NAILONG_MS_CLIENT_ID`）或配置文件注入，仓库只放占位符。

> 该项目需要项目方自己的 client id。**不要**复用 Mojang 官方启动器、PCL、Prism 的 client id（违反条款且可能被封）。
> 设计上 `client_id` 可配置，便于 fork/自建；未获批的 fork 只能先用离线模式。

### 2.2 授权码 + PKCE（默认）

```text
1. 生成 code_verifier：64 字节随机 → base64url（43–128 字符）
   code_challenge = base64url(SHA256(verifier))
   state = 32 字节随机 → base64url
2. 启动本地 HTTP 服务：127.0.0.1:<port>，回调路径 /callback
3. 打开系统浏览器：
   https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize
     ?client_id=<id>
     &response_type=code
     &redirect_uri=http://localhost:<port>/callback
     &response_mode=query
     &scope=XboxLive.signin%20offline_access
     &prompt=select_account
     &state=<state>
     &code_challenge=<challenge>
     &code_challenge_method=S256
4. 回调校验 state（恒定时间比较）；错误参数 → 失败
5. POST https://login.microsoftonline.com/consumers/oauth2/v2.0/token
     grant_type=authorization_code
     client_id=<id>
     code=<code>
     redirect_uri=http://localhost:<port>/callback
     code_verifier=<verifier>
     scope=XboxLive.signin offline_access
6. 得到 { access_token, refresh_token, expires_in }
7. 关闭本地服务（无论成功失败）
```

- 回调页面返回简单 HTML（"登录完成，可关闭此页"），不包含任何令牌。
- 本地服务只监听 `127.0.0.1`，不监听 `0.0.0.0`；5 分钟超时。
- 端口选择：优先固定 `39123`（失败则系统分配）；若使用动态端口，Azure 的 `http://localhost` 注册需允许任意端口。
- `state` 防 CSRF；缺失/不匹配一律拒绝。
- 不使用 `client_secret`；不把令牌写入浏览器 URL。

### 2.3 设备码（兜底）

适用：无浏览器、远程/无图形环境、回环端口被占用、用户选择"用代码登录"。

```text
POST https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode
  client_id=<id>&scope=XboxLive.signin%20offline_access
→ { device_code, user_code, verification_uri, expires_in, interval }

展示 user_code + verification_uri，并打开浏览器
轮询 POST .../oauth2/v2.0/token
  grant_type=urn:ietf:params:oauth:grant-type:device_code
  client_id=<id>&device_code=<device_code>
```

轮询规则：

| 响应 | 行为 |
|---|---|
| `authorization_pending` | 继续，按 `interval` |
| `slow_down` | `interval += 5s` |
| `expired_token` | 失败（提示重新开始） |
| `authorization_declined` | 用户拒绝 |
| 成功 | 同授权码流程 |

总超时 `min(expires_in, 15 min)`；UI 可取消。

### 2.4 Xbox → Minecraft 链路

```text
MSA access token
  │
  ├─ POST https://user.auth.xboxlive.com/user/authenticate
  │   headers: x-xbl-contract-version: 1, Content-Type: application/json
  │   body: {
  │     "Properties": {
  │       "AuthMethod": "RPS",
  │       "SiteName": "user.auth.xboxlive.com",
  │       "RpsTicket": "d=<MSA access token>"
  │     },
  │     "RelyingParty": "http://auth.xboxlive.com",
  │     "TokenType": "JWT"
  │   }
  │   → Token, DisplayClaims.xui[0].uhs
  │
  ├─ POST https://xsts.auth.xboxlive.com/xsts/authorize
  │   headers: x-xbl-contract-version: 1
  │   body: {
  │     "Properties": { "SandboxId": "RETAIL", "UserTokens": ["<XBL Token>"] },
  │     "RelyingParty": "rp://api.minecraftservices.com/",
  │     "TokenType": "JWT"
  │   }
  │   → Token, DisplayClaims.xui[0].uhs, .xid（xuid）
  │
  ├─ POST https://api.minecraftservices.com/authentication/login_with_xbox
  │   body: { "identityToken": "XBL3.0 x=<uhs>;<XSTS Token>" }
  │   → { username, access_token, token_type, expires_in }
  │
  ├─ GET https://api.minecraftservices.com/entitlements/mcstore
  │   Authorization: Bearer <MC access token>
  │   → items[]；为空 = 没有游戏所有权
  │
  └─ GET https://api.minecraftservices.com/minecraft/profile
      Authorization: Bearer <MC access token>
      → { id, name, skins[], capes[] }
```

- UUID 使用 profile 的 `id`（无连字符），不是 MSA 的 id。
- `xuid` 来自 XSTS `DisplayClaims.xui[0].xid`；启动参数 `--xuid` 使用它。
- `clientid` 使用我们的 client id（公开）。
- 令牌有效期：MSA 通常 1 h；MC 通常 24 h。到期前 5 分钟刷新。
- 刷新：用 refresh_token 重新走 MSA → XBL → XSTS → MC 全链路（Xbox 令牌不可跨 MSA 刷新复用）。微软可能轮换 refresh_token，必须落盘新值。
- 刷新后 profile id 变化 → 拒绝（防止登录到另一个账户）。

### 2.5 错误映射

| 来源 | 标识 | 用户提示（中文） |
|---|---|---|
| OAuth | `invalid_grant` / `AADSTS70000` | 登录已过期，请重新登录 |
| OAuth | `access_denied` | 已取消登录 |
| OAuth | `expired_token` | 验证码已过期，请重试 |
| XSTS 401 | `2148916233` | 该微软账户尚未创建 Xbox 档案 |
| XSTS 401 | `2148916235` | 当前地区不支持 Xbox Live |
| XSTS 401 | `2148916238` | 儿童账户需要加入家庭组 |
| XSTS 401 | `2148916227` | 账户已被 Xbox Live 封禁 |
| MC entitlement | items 为空 | 该账户未拥有 Minecraft: Java Edition（Game Pass 到期/未购买）；默认阻止在线启动，提供“仍要尝试”高级选项 |
| MC profile | 404 | 找不到游戏档案（可能未购买/未创建角色） |
| MC login | 403 `Invalid app registration` | 启动器的 client id 未获 Minecraft Services 授权，不是用户问题；显示申请指引 |
| MC login | 401/403 其他 | 登录被拒绝；429 稍后再试，503 Mojang 故障 |

- 原始错误码写入日志（不含令牌），UI 显示可操作建议 + "复制诊断信息"。
- 不自动重试需要用户操作的错误（无 Xbox 档案、未购买、地区限制）。

### 2.6 启动参数

```text
--username <profile.name>
--uuid <profile.id>            # 无连字符
--accessToken <MC access token>
--clientId <client id>
--xuid <xuid 或空>
--userType msa
--userProperties {}
```

- 现代版本 manifest 已包含这些参数，只做变量替换；不要重复追加。
- 离线账户同样填 `--userType msa`（部分服务器对 `legacy` 报 invalid session）；`--accessToken` 使用随机值或 `0`。
- 令牌只在内存与钥匙串中出现；**绝不**写入 `instance.toml`、日志或崩溃报告。

## 3. 令牌存储

### 3.1 钥匙串

| 平台 | 后端 |
|---|---|
| Windows | Credential Manager（`keyring` windows-native） |
| macOS | Keychain（apple-native） |
| Linux | Secret Service（GNOME Keyring/KWallet）；无桌面环境时失败 |

```rust
keyring::Entry::new("nailong", &format!("account/{}", account.id))?
```

- 读取失败（未解锁/无服务）→ 提示"无法访问系统钥匙串"，允许一次性登录但不持久化（会话结束即失效）。
- Linux headless：降级到 `secrets.json`（权限 0600），UI 明确警告"未加密存储"。
- 不把令牌放到环境变量、命令行（`ps` 可见）或 URL。

### 3.2 刷新与并发

- 每个账户一把异步锁，避免启动/设置页同时刷新导致 refresh_token 轮换冲突。
- 刷新流程：读 secret → 若 MC token 仍有效（>5 min）直接使用 → 否则 refresh → 写回。
- 刷新失败：标记 `invalid`，UI 提示重新登录；不删除元数据。
- 账户删除：删除钥匙串条目 + 元数据；失败时至少删除元数据并提示。

### 3.3 脱敏

需要脱敏的字符串：

- MSA access_token、refresh_token
- MC access_token
- authlib access_token
- 任何 `--accessToken` 后面的值

实现：日志层统一 `Redactor`，按值替换；同时正则兜底 `-{0,2}accessToken[= ]\S+`。崩溃报告/诊断包发送前再次扫描。

## 4. 离线账户

Java 的 `UUID.nameUUIDFromBytes` **不是**标准的 namespace-based UUID v3；它是对原始字节做 MD5，再设置版本/变体位。必须按字节复刻，不能用 `Uuid::new_v3`（后者会拼接命名空间，结果不同）：

```rust
let digest = md5::Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
let mut b = [0u8; 16];
b.copy_from_slice(&digest[..16]);
b[6] = (b[6] & 0x0f) | 0x30;   // version = 3
b[8] = (b[8] & 0x3f) | 0x80;   // variant = RFC 4122
let uuid = Uuid::from_bytes(b);
```

```text
name: 3–16 字符，[A-Za-z0-9_]
accessToken: 随机 32 hex（或 "0"；部分启动器用 UUID）
userType: msa（部分服务器对 legacy 报 invalid session）
```

- 离线账户仅用于单人/局域网/允许离线模式的服务器；不绕过正版服务器验证。
- 同名离线账户在不同设备上 UUID 一致（便于存档/服务器白名单）。
- 用户名冲突：允许重名（UUID 不同）但 UI 警告。
- 皮肤：默认 Steve/Alex；自定义皮肤需要本地 Yggdrasil 服务（v2）。

## 5. authlib-injector（外置登录，v1.1）

### 5.1 准备

1. 从官方发布或镜像下载 `authlib-injector-<version>.jar`。
2. 校验 SHA256（内置已知版本哈希），存 `shared/authlib-injector/<version>.jar`。
3. 获取服务器元数据：`GET <server>/`（Yggdrasil API 根）→ JSON，含 `signaturePublickey`、`skinDomains`、`meta`。

### 5.2 登录

```text
POST <server>/authserver/authenticate
{
  "username": "...",
  "password": "...",
  "clientToken": "<本地随机>",
  "agent": { "name": "Minecraft", "version": 1 },
  "requestUser": false
}
→ { accessToken, clientToken, selectedProfile: { id, name }, availableProfiles: [...] }
```

- 密码只在内存中使用一次，不落盘、不日志。
- 刷新：`POST /authserver/refresh`（accessToken + clientToken）。
- 验证：`POST /authserver/validate`。
- 皮肤：`GET <server>/sessionserver/session/minecraft/profile/<uuid>` → textures 属性（base64）。

### 5.3 启动注入

```text
-javaagent:<authlib-injector.jar>=<server>
-Dauthlibinjector.side=client
-Dauthlibinjector.yggdrasil.prefetched=<base64(metadata JSON)>
```

- 三个参数都加；`prefetched` 可减少游戏启动时的网络请求与 TLS 问题。
- 服务器地址必须 HTTPS（默认）；HTTP 需用户显式确认（局域网/自建）。
- 令牌存钥匙串（kind = Authlib）。
- 账户 UI 显示服务器名称/域名，避免钓鱼混淆。

## 6. 皮肤与头像

### 6.1 微软皮肤

- `profile.skins[]` 的 `url` 下载 PNG（64×64 或 64×32），按 URL 哈希缓存到 `cache/icons/skins/`。
- 2D 头像：裁剪头部（8,8,8,8）+ 帽子层（40,8,8,8），缩放显示；不需要 3D 渲染。
- `variant` 决定 Classic/Slim（仅影响 3D，2D 不影响）。
- 披风：可选显示（v1.1）。

### 6.2 离线/外置

- 默认皮肤：内置 Steve/Alex 头像（原创绘制或许可证兼容素材）。
- authlib 皮肤：从会话服务器获取；失败回退默认。
- 自定义本地皮肤（v2）：需要本地 Yggdrasil 服务 + 签名，复杂度高，不在 v1。

## 7. UI 流程

```text
设置 → 账户
  [使用 Microsoft 登录]  → 浏览器/设备码 → 成功后回到列表
  [添加离线账户]         → 输入名 → 创建
  [添加外置登录]         → 服务器地址 + 用户名/密码 → 登录
列表：头像、名字、类型徽标、当前账户标记、刷新/删除
```

- 登录中显示进度与取消；设备码显示大号 user_code 与复制按钮。
- 令牌过期时，实例列表的"启动"旁显示"账户需要重新登录"。
- 多账户：全局当前账户；实例可覆盖（v1.1）。

## 8. 安全要点

| 风险 | 缓解 |
|---|---|
| CSRF/授权码拦截 | PKCE S256 + state + 回环地址 |
| 令牌泄漏到日志 | 统一 Redactor；测试断言 |
| 令牌被其他进程读取 | 系统钥匙串；降级文件 0600 且明示 |
| 钓鱼外置登录 | 显示完整域名；HTTPS 默认；警告未知证书 |
| 密码留存 | 不持久化，只在内存；登录后立即丢弃 |
| 刷新竞争 | 账户级异步锁；轮换后原子写 |
| 回调端口被占用 | 固定端口失败则动态端口；设备码兜底 |

## 9. 测试

| 测试 | 断言 |
|---|---|
| PKCE | challenge = base64url(sha256(verifier))；verifier 长度合法 |
| 授权码 | state 不匹配拒绝；缺失 code 拒绝；超时关闭服务 |
| 设备码 | pending → success；slow_down 增加间隔；expired 失败 |
| Xbox | 各错误码映射正确；请求体字段正确 |
| 刷新 | 轮换 refresh_token 写回；profile id 变化拒绝 |
| 钥匙串失败 | 降级路径与警告；不崩溃 |
| 脱敏 | 日志/事件/导出脚本均不含令牌 |
| 离线 UUID | 与 Java `UUID.nameUUIDFromBytes` 已知向量一致 |
| authlib | 三参数注入；密码不落盘 |

## 10. 待确认

- 项目方 Azure 应用注册由谁完成、client id 如何分发（编译期常量 vs 配置文件）。
- 是否支持组织账户（`common`）而非仅 `consumers`。
- Linux 无 Secret Service 时的默认策略（拒绝持久化 vs 明文文件 + 警告）。
- 是否在 v1 支持多账户并存（建议支持，成本低）。
