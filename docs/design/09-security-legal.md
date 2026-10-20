# 09 · 安全、隐私与合规

> 启动器是"下载并执行第三方代码"的软件：Minecraft、Forge processors、模组、Java 运行时都来自网络。
> 目标不是"绝对安全"，而是**边界清晰、默认保守、失败可控、责任可追溯**。

## 1. 资产与信任边界

| 资产 | 风险 | 保护 |
|---|---|---|
| 微软 refresh/access token | 账户被盗、被封 | 系统钥匙串；不落日志；不写 argv 以外的持久化位置 |
| 玩家存档/配置 | 丢失/损坏 | 原子写；删除需二次确认；导入不覆盖用户文件 |
| 启动器二进制 | 被替换/植入 | 代码签名（后续）；自动更新签名校验 |
| 下载内容 | 木马/损坏 | TLS + 哈希校验 + 大小校验 |
| 本地文件系统 | 路径穿越/覆盖 | 规范化 + 根目录约束 |
| 用户机器 | 任意代码执行（Forge/Java） | 只从可信源安装；校验；最小权限；不静默提权 |

信任边界：

```text
[用户] → [nailong UI] → [nailong-core] → [HTTPS 源] → [本地磁盘] → [Java 进程/Minecraft/模组]
```

- 启动器**不**以管理员/root 运行；不请求提权。
- Java 进程以当前用户权限运行；模组可访问用户文件（这是 Minecraft 生态的既有事实，不做虚假承诺）。
- 启动器只对"自己下载并校验的文件"负责；用户手动放入的 jar/整合包由用户信任。

## 2. 威胁模型

| # | 威胁 | 场景 | 缓解 |
|---|---|---|---|
| T1 | 镜像/中间人返回恶意文件 | 下载库/客户端/资源 | HTTPS；哈希校验（SHA1/SHA256）；大小校验；失败换源 |
| T2 | 版本 JSON 指向恶意路径 | `downloads.artifact.path = ../../...` | 路径规范化；拒绝绝对/`..`；目标必须在根内 |
| T3 | 整合包 zip 路径穿越 | `../../.bashrc` | 条目校验；规范化；拒绝符号链接；前缀检查 |
| T4 | Zip bomb | 极小压缩包解压数百 GB | 条目数/总大小/压缩比限制；流式计数 |
| T5 | 恶意 Forge installer/processor | 执行任意 Java 代码 | 仅从官方 maven 下载；校验 install_profile 中哈希；临时目录；不传令牌；提示风险 |
| T6 | 恶意模组 | 游戏内任意代码 | 超出启动器边界；v2 评估沙盒；文档明确 |
| T7 | 令牌泄漏 | 日志/崩溃报告/导出脚本 | 统一 Redactor；argv 风险提示；v2 Java shim |
| T8 | 本地其他用户读取令牌 | 共享机器 | 钥匙串；降级文件 0600；不写临时文件 |
| T9 | 下载 URL SSRF | 整合包/API 返回内网 URL | 仅 HTTPS；拒绝私有 IP/回环；重定向每跳检查 |
| T10 | 供应链攻击 | crate 被投毒 | Cargo.lock；cargo-deny/audit；最小依赖；审查 build.rs |
| T11 | 自动更新被劫持 | 替换二进制 | 签名清单 + 公钥内置；TLS；回滚保护 |
| T12 | 权限提升 | 以管理员运行 | 不请求提权；安装目录用户可写；安装包不写系统目录 |
| T13 | 拒绝服务（本地） | 超大日志/无限重试 | 环形缓冲；重试上限；磁盘配额提示 |
| T14 | 钓鱼外置登录 | 用户输入密码到假服务器 | 显示完整域名；HTTPS 默认；警告 |

明确**不在** v1 防护范围：

- 恶意模组对用户文件的访问（Minecraft 模组本身拥有用户权限）。
- 服务器端漏洞/外挂。
- 物理访问/已失陷的操作系统。
- 供应链的零日（只能通过及时更新降低）。

## 3. 网络与下载

### 3.1 传输

- 默认仅 HTTPS；`http://` 只允许 `localhost`（测试/本地镜像）并显式警告。
- 证书校验使用 reqwest 0.13 的 `rustls` + `rustls-platform-verifier`（系统根证书）；不提供"忽略证书错误"开关。企业代理可把根证书导入系统信任库。
- 重定向：最多 10 跳；每跳重新校验协议与主机；拒绝 HTTPS → HTTP 降级。
- 超时：连接 10 s，读取 30 s（大文件按空闲超时）。
- User-Agent：`nailong/<version> (<os>; <arch>)`，便于服务方识别与限流。
- 代理：支持系统代理/手动 HTTP/SOCKS5；认证信息不写日志。

### 3.2 完整性

- 有哈希必校验：SHA1（Mojang）、SHA256/SHA512（Fabric/Modrinth）、MD5（少数旧库）。
- 流式校验：下载同时计算，避免二次读盘；校验通过才 `rename` 到目标。
- 无哈希的文件：至少校验大小 + Content-Length；记录"未校验"状态。
- 资源对象以哈希命名，存在性即内容一致性的弱保证；`verify` 做深度校验。
- 镜像返回 HTML/错误页：检查 Content-Type 与解析结果，失败换源。

### 3.3 镜像信任

- 内置镜像（BMCLAPI 等）视为**传输加速**，不是信任根：仍以官方哈希校验。
- 自定义镜像：用户自担风险；仍校验哈希；UI 提示。
- 不在镜像间共享 cookie/凭据；下载请求不携带认证头（除非同源 API）。

### 3.4 SSRF 防护

- 整合包/API 返回的下载 URL：只允许 `https`；解析主机名后拒绝回环/私有/链路本地地址（`127/8`、`10/8`、`172.16/12`、`192.168/16`、`169.254/16`、`::1`、`fc00::/7`、`fe80::/10`）。
- 重定向每跳重新检查。
- 不允许 `file:`、`data:`、`ftp:` 等 scheme。
- 残余风险：DNS rebinding；v1 接受并在文档说明，后续可固定解析 IP。

## 4. 压缩包与文件系统

### 4.1 解压防护

| 检查 | 限制（默认） |
|---|---|
| 条目数 | ≤ 100,000 |
| 单文件解压后大小 | ≤ 2 GB |
| 总解压大小 | ≤ 20 GB（整合包可配置） |
| 压缩比 | ≤ 200:1（超过则拒绝并提示） |
| 路径 | 相对、无 `..`、无盘符、无 NUL、无前导 `/` |
| 符号链接/硬链接 | 拒绝 |
| 加密条目 | 拒绝（不支持密码） |
| 覆盖 | 只允许写入实例目录/目标根；不覆盖 `.toml` 等控制文件之外的系统文件 |

实现：

```rust
fn safe_join(root: &Path, entry: &str) -> Result<PathBuf> {
    let rel = Path::new(entry);
    if rel.is_absolute() { return Err(UnsafeArchiveEntry); }
    let mut out = root.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => return Err(UnsafeArchiveEntry),
        }
    }
    // 规范化后再次确认前缀（防止 Windows 8.3/大小写/UNC 边缘情况）
    let canon_root = root.canonicalize()?;
    let parent = out.parent().unwrap_or(root);
    std::fs::create_dir_all(parent)?;
    let canon_parent = parent.canonicalize()?;
    if !canon_parent.starts_with(&canon_root) { return Err(UnsafeArchiveEntry); }
    Ok(out)
}
```

- 解压到临时目录，成功后原子合并；失败清理。
- 不跟随目标位置的符号链接（先 `symlink_metadata`）。

### 4.2 版本 JSON 路径

- `downloads.artifact.path` / maven 坐标推导的路径：必须相对、无 `..`；拼接后仍在 `libraries/` 内。
- `jar` 字段只能是版本 id，不允许路径。
- `assetIndex.id` 清洗为文件名安全字符。
- 本地自定义 profile：用户显式指定路径，仍限制在数据根内（或明确警告）。

### 4.3 文件权限

- 数据目录：用户私有（Unix 0700，文件 0600；Windows 继承用户 ACL）。
- 降级令牌文件：0600；启动时检查权限，过宽则警告并收紧。
- 导出的启动脚本含令牌：0600，并提示"请勿分享"。

## 5. 代码执行

### 5.1 Forge / NeoForge processors

- 只从官方 maven（或用户选择的可信镜像）下载 installer 与 processor jar。
- 校验 install_profile 中声明的哈希（若有）；无哈希时记录警告。
- 在**临时 staging 目录**运行，`ROOT` 指向 staging；成功后原子移入。
- 不把访问令牌、账户信息传入 processor 环境；清理敏感环境变量。
- 不以管理员运行；不修改系统目录。
- 每个 processor 超时（10 min）与输出大小限制（防止日志爆炸）。
- 失败保留日志，UI 提示"可改用官方安装器"（用户显式操作）。

### 5.2 Java agents / authlib-injector

- 只允许用户显式添加的 agent；UI 显示路径与来源。
- authlib-injector：校验内置版本哈希；服务器地址 HTTPS；密码不落盘。
- 不对 agent 做沙盒（Java agent 拥有完整权限）；文档说明。

### 5.3 启动进程

- 绝不经过 shell；`Command` 直接传 argv。
- 不执行来自版本 JSON 的任意命令；只执行 `java`（用户选择/托管的路径）。
- `java` 路径必须是文件且可执行；拒绝目录/符号链接指向系统敏感路径？符号链接可接受（用户环境），但记录 realpath。
- 环境变量：移除 `JAVA_TOOL_OPTIONS` 等注入项（默认）；用户可在高级设置恢复。
- 工作目录限制在实例/共享目录；自定义目录需用户确认。

## 6. 凭据与隐私

### 6.1 令牌

- 存储在系统钥匙串；Linux 无 Secret Service 时降级文件（0600 + 明确警告）。
- 内存中使用 `secrecy`/`zeroize`（可选）减少残留；不派生 `Debug` 输出。
- 日志：统一 Redactor，按值替换；测试断言 token 不出现在任何日志/事件/导出文件。
- 错误信息：不包含请求头/响应体中的令牌。
- 崩溃报告：发送前再次扫描（v1 仅本地查看）。

### 6.2 遥测

- v1 **默认无遥测**；不联网上报使用数据。
- 若未来加入匿名统计：显式 opt-in、可查看内容、可关闭、不上传路径/账户/日志。
- 更新检查只请求版本号（可关闭）。

### 6.3 本地数据

- 日志保留 10 次游戏日志 + 5 个启动器日志；用户可一键清理。
- 诊断包（用户主动导出）：包含版本/Java/错误码/最近日志（脱敏）；明确列出内容。

## 7. 自动更新（v1.1）

- 更新清单（JSON）由项目签名（Ed25519/minisign）；公钥编译进二进制。
- 下载产物校验 SHA256 + 签名；校验失败拒绝安装。
- 更新器只替换自身二进制/资源，不执行任意脚本；Windows 使用"重命名后替换"策略。
- 支持回滚（保留上一版本）；灰度/暂停开关。
- 不通过 HTTP 分发；不接受镜像的更新包（除非签名一致）。

## 8. 许可证与洁净室

### 8.1 参考项目的许可证

| 项目 | 许可证 | 能否复制代码 |
|---|---|---|
| Prism Launcher | GPL-3.0 | 仅当 nailong 也以 GPL-3.0 兼容方式发布，并履行义务 |
| HMCL | GPL-3.0 | 同上 |
| PCL | 《PCL 分发有限许可》 | **不能**；连衍生作品都可能受限 |
| portablemc | Apache-2.0 | 可以（保留 NOTICE） |
| LightyLauncherLib | MIT | 可以（保留版权） |
| Pandora | MIT | 可以 |
| egui / tokio / reqwest | MIT/Apache-2.0 | 可以 |

### 8.2 已定：GPL-3.0-or-later

2026-10-20 决定：项目采用 **GPL-3.0-or-later**。

- 可以在履行 GPL 义务的前提下参考/移植 Prism、HMCL 等 GPLv3 项目的实现（保留版权声明、标注来源、提供完整源码）。
- 不再需要"洁净室"流程；但仍不得复制 PCL 的受限代码/资源。
- 依赖必须与 GPLv3 兼容：MIT/Apache-2.0/BSD 可以；专有/受限许可证需评审。
- 发布时附 `LICENSE`（GPLv3 全文）、`THIRD_PARTY_NOTICES.md`、以及修改过的 GPL 代码来源说明。
- 若未来需要闭源分发，只能重写所有来自 GPL 项目的部分；本项目不预留该路径。

> **当前设计文档只描述行为与公开规范，不包含参考项目代码**。

### 8.3 PCL 的特殊限制

- PCL 的许可证不是 OSI 开源许可，禁止未经授权的分发/修改/衍生。
- 本项目**不复制 PCL 代码**；仅研究其公开行为与用户界面流程。
- 不在文档/代码中复制 PCL 的图标、文案、资源。

### 8.4 名称与素材

- 项目名 **nailong** 由作者本人使用。
- 素材合规：应用图标、启动画面、插画、字体必须是原创或使用可再分发许可证的素材。
- 不使用任何第三方角色/动画截图作为 Logo 或宣传图；不使用来源不明的字体。
- 发布包内保留素材的源文件与许可证说明。

### 8.5 字体与图标

| 资源 | 许可证 | 义务 |
|---|---|---|
| Noto Sans SC（子集） | OFL-1.1 | 保留版权与许可证；不得单独售卖字体 |
| Inter | OFL-1.1 | 同上 |
| Phosphor Icons | MIT | 保留版权声明 |
| 自绘图标 | 项目许可证 | — |

- 发布包内附 `THIRD_PARTY_NOTICES.md`；CI 检查许可证文件存在。

### 8.6 Mojang / Microsoft

- 不分发 Minecraft 客户端、资源、库文件；全部从官方/授权源下载。
- 不绕过正版验证；离线模式仅用于合法场景。
- 不声称与 Mojang/Microsoft 有关联；关于页注明。
- 遵守 Minecraft EULA 与使用指南；不将游戏素材用于商业宣传。
- 微软 OAuth 应用遵守 Microsoft 身份平台条款；不冒用官方 client id。

## 9. 第三方服务合规

| 服务 | 要求 | 我们的做法 |
|---|---|---|
| Mojang piston-meta | 合理频率、User-Agent | 缓存 + 条件请求；不爬全量 |
| BMCLAPI | 遵守其使用说明、不要滥用 | 作为可选镜像；失败回退官方；不绕过限流 |
| Fabric/Quilt Meta | 合理频率 | 缓存 1 小时 |
| Modrinth API | 标识 User-Agent、遵守速率限制 | 只读、缓存；不批量抓取 |
| CurseForge API | 需要 API key、遵守条款 | 用户自备 key；不内置泄漏 |
| Microsoft OAuth | public client、不共享 secret；Minecraft Services 需 client id allowlist | PKCE/设备码；client id 可配置；M0 提交审批，未获批不宣称正版登录 |

## 10. 供应链与发布

- 提交 `Cargo.lock`；CI 使用 `--locked`。
- `cargo-deny`：拒绝未知许可证、重复版本、已知漏洞（RUSTSEC）。
- 审查 `build.rs`；优先无构建脚本的 crate。
- 发布产物附带 SHA256 清单与 SBOM（`cargo cyclonedx`，v1.1）。
- Windows 代码签名、macOS 公证（预算允许时）；未签名版本在下载页显著说明。
- 构建可复现：固定工具链、`SOURCE_DATE_EPOCH`（尽量）。

## 11. 安全测试清单

| 测试 | 断言 |
|---|---|
| 路径穿越 | `../`、绝对路径、UNC、Windows 设备名（CON/NUL）被拒绝 |
| Zip bomb | 超过条目/大小/压缩比被拒绝，内存不爆 |
| 符号链接 | 压缩包内 symlink 被拒绝 |
| 恶意 JSON | 路径字段越界被拒绝；URL scheme 非 HTTPS 被拒绝 |
| SSRF | 私网/回环 URL 被拒绝（含重定向） |
| 令牌泄漏 | 日志、事件、导出脚本、错误信息均不含令牌 |
| 权限 | 降级令牌文件 0600；过宽权限被收紧/警告 |
| 下载校验 | 错误哈希不落盘；临时文件清理 |
| Processor | 非 0 退出、超时、输出过大均被处理；不传令牌环境 |
| 重定向 | HTTPS→HTTP 被拒绝；循环被终止 |
| 更新 | 签名错误/哈希不符拒绝安装 |

## 12. 待决策

1. ~~最终许可证~~ → 已定 GPL-3.0-or-later。
2. 是否内置 CurseForge API key（不建议）或要求用户配置。
3. 是否在 v1 引入遥测（建议否）。
4. 代码签名/公证预算。
5. 图标/插画素材（原创或合规来源）。
