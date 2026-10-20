# 01 · 先行研究：Prism / HMCL / PCL 与 Rust 竞品

> 研究时间：2026-10-20。仓库均为浅克隆只读分析；结论附文件/类名证据。
> 目的：明确 nailong **抄什么、不抄什么、如何差异化**。

## 1. 研究范围

| 项目 | 语言/UI | 许可证 | 研究版本 |
|---|---|---|---|
| [Prism Launcher](https://github.com/PrismLauncher/PrismLauncher) | C++ / Qt 6 | GPL-3.0 | `develop` @ 77f2bee |
| [HMCL](https://github.com/HMCL-dev/HMCL) | Java 17 / JavaFX | GPL-3.0 | HEAD @ 8827fd0 |
| [PCL](https://github.com/Meloong-Git/PCL) | VB.NET / WPF | **PCL 分发有限许可（非开源）** | 浅克隆 HEAD |
| [PandoraLauncher](https://github.com/Moulberry/PandoraLauncher) | Rust / gpui | MIT | HEAD |
| [QuantumLauncher](https://github.com/Mrmayman/quantumlauncher) | Rust / iced | GPL-3.0 | HEAD |
| [portablemc](https://github.com/theorzr/portablemc) | Rust / CLI | Apache-2.0 | HEAD |
| [LightyLauncherLib](https://github.com/Lighty-Launcher/LightyLauncherLib) | Rust / 库 | MIT | HEAD |

> 许可证是硬约束：Prism/HMCL 的代码不能复制进 MIT/Apache 项目；PCL 连"参考代码"都受限制。详见 [09 §8](09-security-legal.md#8-许可证与洁净室)。

## 2. 横向对比

| 维度 | Prism | HMCL | PCL | **nailong 目标** |
|---|---|---|---|---|
| 版本模型 | 私有 meta 组件（`mmc-pack.json`），**不解析官方 `inheritsFrom`** | 官方 manifest 递归 merge + 自研 patch 合成 | 官方 JSON 直接递归 merge（libraries 拼接） | **官方 profile + `inheritsFrom` 链**（可互操作） |
| 加载器 | meta 预构建包；Forge 用 ForgeWrapper 在游戏 JVM 内装 | 自研安装器；Forge 原生 processors | 自研安装器（含 Forge/NeoForge/OptiFine） | **原生安装器**；Fabric/Quilt JSON，Forge/NeoForge processors |
| 元数据依赖 | 强依赖 `meta.prismlauncher.org` | Mojang + 自建镜像规则 | Mojang + 多源 | **Mojang 官方为唯一必需源**；镜像可选 |
| 下载 | 原子写、校验、无断点续传 | 候选 URL、ETag、Range 续传、全局并发 | 多线程分段、源切换 | 候选 + 续传 + 内容寻址 |
| 任务模型 | `Task` + `LaunchStep` 链 | 双向 DAG + 动态扩展 | 自研 Loader 任务 | 普通 async + 阶段/取消（不做 DAG 引擎） |
| 账户 | device code + loopback；token 明文 `accounts.json` | PKCE + device code；元数据/私钥分离 | 微软 + 外置登录 | PKCE 为主 + 设备码；**钥匙串** |
| UI 依赖 | Qt Widgets/NetworkAuth | JavaFX（core 也被污染） | WPF | **纯 Rust egui**（core 零 GUI） |
| 发布体积（参考） | Windows zip ≈ 19.5 MiB；Linux AppImage ≈ 102.6 MiB；macOS zip ≈ 41.6 MiB | 10–20 MB jar + JRE 17/JavaFX | .NET 单文件（Windows only） | **单 exe ≤ 15 MB** |
| 平台 | Win/macOS/Linux | Win/macOS/Linux | Windows | Win/Linux 优先，macOS 后续 |

## 3. Prism Launcher：元数据服务驱动的组件化

### 3.1 关键事实

- 生产代码**不调用** `MojangVersionFormat::versionFileFromJson`，只走 `OneSixVersionFormat`（`launcher/minecraft/ProfileUtils.cpp`、`launcher/meta/JsonFormat.cpp`）。官方 `arguments`、`logging`、`os.arch`、`features` 在客户端**不被解析**。
- 实例是 `mmc-pack.json` 的**有序组件列表**；`Component` 从 meta 或本地 `patches/<uid>.json` 装载，按顺序 `applyTo` 合并成 `LaunchProfile`（`launcher/minecraft/PackProfile.cpp`、`VersionFile.cpp`）。
- 没有通用 `inheritsFrom` 解析；只有 Technic 导入时读一次。
- Forge/NeoForge 的 `mainClass` 被设为 `io.github.zekerzhayard.forgewrapper.installer.Main`，installer 作为 `mavenFiles` 下载，**processors 在游戏 JVM 内由 ForgeWrapper 执行**，不在启动器里。
- 规则引擎只按 `os.name` 生效；`os.arch`、`features`、`os.version` 缺失（`launcher/minecraft/Rule.h`）。`extract.exclude` 解析但不使用。
- 下载：`FileSink` 原子写 + SHA 校验，**不支持断点续传**；`NetJob` 默认并发 6。
- 账户：device code 与 loopback 两条；token 明文存 `accounts.json`（0600）；启动时通过 Java shim 的 stdin 传递，避免出现在 argv。
- 体积：UI 代码 44,988 行（占 38.5%），Qt 模块带来主要体积；Windows zip 约 19.5 MiB，Linux AppImage 约 102.6 MiB。

### 3.2 值得借鉴

1. **组件可禁用、可本地覆盖**：用户不被 meta 锁死。
2. **按顺序合并、库去重、后写覆盖**：简单可预测。
3. **meta 三层缓存 + sha256 + 条件请求**：离线可用、防篡改。
4. **启动步骤链 + 逆序清理**：失败时能释放已占用资源。
5. **token 不进 argv**（Java shim）：安全性最好，但复杂度高；v1 先用 argv + 脱敏，v2 评估。
6. **原子写 + 校验后提交**。

### 3.3 必须避免

| 反模式 | 后果 | nailong 做法 |
|---|---|---|
| 把"版本是什么"外包给私有 meta | 服务不可用即无法装新版本；新 MC 版本有延迟 | 官方 manifest 为一等输入 |
| 丢掉官方 `arguments`/`logging`/`features` | 新版本/特殊规则失效 | 完整解析，规则引擎补全 arch/features/versionRange |
| Forge 依赖第三方 wrapper | 首次启动慢、错误黑盒、离线不可装 | 原生 processors（v1） |
| 规则只看 `os.name` | 错误选择库/natives | 完整规则 |
| 全局单例 + UI 逻辑进核心 | 无法 headless/测试 | 门面 + 依赖注入 |
| 不支持续传 | 大文件失败重来 | Range + If-Range |

## 4. HMCL：分层最清晰、领域知识最完整

### 4.1 关键事实

- 三层：`HMCLBoot`（Java 8 守卫）→ `HMCL`（JavaFX 应用/编排）→ `HMCLCore`（65.4k 行领域内核），依赖严格单向；但 core 有 35 个文件 import JavaFX（进度属性），并非纯 domain。
- 版本继承：`GameInstanceManifest.merge(parent)`，子优先、`libraries` **子前父后**、`arguments` 父前子后、带环检测。
- 安装即产出 `GameInstancePatch`（带 priority），启动前按优先级合成；落盘"扁平字段 + patches"双写。
- 启动命令拼装顺序是一份完整规格（见 §4.2）；`addDefault` 让所有自动参数可被用户同前缀参数抑制。
- 下载：`DownloadProvider` + 候选 `DownloadCandidates` + `FetchTask`（5 次重试、ETag、Range/If-Range、全局信号量 `min(4*CPU,64)`、`x-bmclapi-hash` 命中）；镜像用 20+ 条 `MirrorRule` 前缀重写，不做 Provider 子类。
- Forge 新格式：临时目录隔离 client.jar、按 `outputs` SHA1 跳过已完成 processor、校验产物、`DOWNLOAD_MOJMAPS` 特判、zlib 兼容例外。
- 账户：PKCE + device code；元数据与私密数据分文件；XBL→XSTS→login_with_xbox→entitlements→profile；authlib-injector 三参数 + 预取 metadata。
- 退出判定：日志关键词（JVM_ERROR/APPLICATION_ERROR/SIGKILL）+ 环形缓冲 + token 脱敏 + 200ms 合批。

### 4.2 启动顺序（可直接作为规格）

```text
优先级前缀 → wrapper → java → 用户覆盖 JVM 参数 → 内存 → 编码 → log4j 缓解
→ 自动优化参数 → 代理 → manifest.arguments.jvm（-cp / java.library.path 来源）
→ 账号注入 → javaagent → mainClass → 旧版 minecraftArguments
→ arguments.game → QuickPlay → 用户游戏参数 → forbidden 过滤
```

### 4.3 值得借鉴

1. **patch 化安装**（不可变增量 + 优先级合成）：卸载/重装/多加载器都自然；nailong 用"profile + 组件元数据"实现等价能力，但**只保留一份真相**，不双写。
2. **候选列表 + 规则表的镜像**：新增镜像 = 加一行。
3. **保留原始 JSON**：未知字段不丢。
4. **Layout + run directory 解析函数**：隔离不是布尔字段。
5. **可解释退出 + 固定日志契约**。
6. **Forge processor 的工程细节**（临时 jar、outputs 跳过、产物校验）。

### 4.4 必须避免

| 反模式 | nailong 做法 |
|---|---|
| core 绑 JavaFX 属性 | core 零 GUI；`watch`/事件 |
| 任务图在执行期动态生成 | 静态 async 阶段；取消令牌 |
| patches 与扁平字段双写 | profile 文件为唯一真相；resolved 只是缓存 |
| 80 行 `instanceof` 拼错误文案 | 错误码 + i18n 表 |
| 运行时必须 JRE 17 + JavaFX | 单二进制，无运行时依赖 |

## 5. PCL：最务实的"单文件"路线

### 5.1 关键事实

- VB.NET / WPF，Windows only，目标 **.NET Framework 4.8**（Win10/11 自带）；源码按发布同步，**许可证为《PCL 分发有限许可》**，不是 OSI 开源许可。
- 规模：113 个 `.vb` 共 **42,631 行**，63 个 `.xaml` 共 5,615 行；`Modules/Minecraft` 7 个文件约 8.7k 行（`ModMinecraft`/`ModLaunch`/`ModDownload`/`ModJava`/`ModModpack`/`ModCrash`/`ModWatcher`），`Modules/Base`（`ModNet` 1.9k、`ModBase` 1.2k、`ModLoader` 791）承担运行时。
- 发布物：`Plain Craft Launcher 2.exe` **5.70 MiB**，zip 2.54 MiB。它的"小"主要来自**框架依赖**（不打包 .NET）+ 依赖 DLL 以 `EmbeddedResource` 存放、运行时 `AssemblyResolve` 加载，而不是"单文件发布"本身。
- 版本模型：官方 `version.json` 优先 + `inheritsFrom` 递归合并，兼容 HMCL 的 `patches`；不建私有 meta，也不把 Forge 安装外包给 wrapper。
- 规则引擎是 **Windows 特化**：主要认 `os.name=windows`，忽略 `os.version`，`features` 只硬编码 demo/quick play；不可移植（Prism 更弱，HMCL 最全）。
- 启动：`McLaunchStart` 是一条带 `ProgressWeight` 的加载器流水线（获取 Java/登录/补全文件可并行，参数/natives/启动串行）。参数以**字符串拼接 + 引号**再 `SplitJavaArguments` 去重，务实但脆弱。
- 下载：`ModNet` 的多源分段下载非常成熟——默认 **64 线程**、256 KiB 分片、按源隔离故障/禁用、Range 不支持时整体回退单线程、低速断链、跨版本按大小+SHA1 复用文件；BMCLAPI 403/429 特殊处理。
- 登录：标准 device code 六步（MSA→XBL→XSTS→`login_with_xbox`→entitlements→profile），错误码映射完整；**token 明文存 ini**，client id 从环境变量 `PCL_MS_CLIENT_ID` 读取（不写进仓库）。
- 轻量手段：框架依赖 + 嵌入程序集 + 便携目录 + 自绘 WPF 控件；代价是 Windows only 与 .NET 运行时。

### 5.2 值得借鉴

1. **预检查 + 内存优化 + 一键修复**的务实顺序。
2. **多源分段下载**：源级熔断、Range 合并、失败重试。
3. **版本 JSON 缺失时从 jar 内恢复**（健壮性）。
4. **面向国内网络的镜像与错误提示**。
5. **加载器树 + 权重进度**：`LoaderTask / LoaderCombo / LoaderDownload` 可嵌套、可取消、带权重，启动流水线/下载/扫描复用；比 Prism 的串行 Task 链更容易做"总进度不倒退"。
6. **极简发布**：5.7 MiB 单 exe 证明"启动器不必是几百 MB"，但前提是接受框架依赖（Win10/11 自带）。

### 5.3 不可借鉴

- 许可证限制：**不能复制代码**，只能研究行为。
- 字符串拼参数再切分：引号/空格/中文易错；nailong 用 argv 数组。
- Windows-only 的 WPF 架构。
- 所有逻辑堆在少数大文件里（可维护性）。

## 6. Rust 竞品

### 6.1 生态现状（2026-10）

| 项目 | 技术栈 | 特点 | 对我们的意义 |
|---|---|---|---|
| **PandoraLauncher** | Rust + **gpui** + gpui-component；workspace（backend/bridge/frontend/schema/auth）；MIT | 原生 UI、无 Electron/Tauri；Windows 便携包 **53.4 MB**；钥匙串；mod 去重；token 日志脱敏 | 证明 gpui 可做精致启动器；但体积/编译/学习曲线不适合轻量首发 |
| **QuantumLauncher** | Rust + **iced 0.13**；GPL-3 | Windows zip **8.1 MB**；自称"比任务管理器还轻"；实例隔离、mod store、旧版本支持 | 证明 iced 可做轻量 GUI；但自定义视觉与中文排版成本高 |
| **Nitrolaunch** | Rust；GPL-3；CLI + GUI；模块化实例模板 | 0.32.x，长期维护；服务端管理 | 模块化/模板思路可参考 |
| **portablemc** | Rust；Apache-2.0；CLI/库/FFI/Python | 5.0.6；成熟的版本解析/安装/认证 | **最接近我们的 core 形态**；可作为行为对照 |
| **LightyLauncherLib** | Rust；MIT；crates：core/auth/java/launch/loaders/version | 支持 Fabric/Quilt/Forge/NeoForge；docs.rs | 验证了"库 + 多 crate"拆分；可参考 API 设计 |
| **OneLauncher** | Rust + Freya/Skia；GPL-3；100% Rust | Windows exe **19.7 MB**；Polyfrost 生态；安装包/自动更新 | 分发与更新工程参考；体积高于 egui/iced |

> 结论：Rust 做 MC 启动器在 2026 年已被验证可行，但**没有成熟到可以闭眼依赖的启动器框架**——生产项目（Modrinth/Pandora/LiquidLauncher）都自研 core；`minecraft-auth`、`mc-launcher-core` 等 crate 仍很年轻。同时做到"单二进制、无 WebView、原生 processors、完整中文体验"的项目很少，nailong 的差异化空间存在。

### 6.2 从竞品得到的三个判断

1. **GUI 不是瓶颈，领域正确性才是**：多数项目卡在 Forge/OptiFine/旧版本/资源补全，而不是界面。
2. **不要依赖私有 meta**：Prism 的教训；官方 JSON + 少量加载器端点足够。
3. **"轻量"是系统工程**：单二进制、零运行时、低内存、快启动、下载不卡 UI，需要从架构第一行代码就约束。

## 7. 十条结论

### 要抄

1. **官方 version JSON 为一等输入**，完整解析 `arguments`/`logging`/`rules`/`javaVersion`。
2. **profile + `inheritsFrom` 链**作为版本组合方式，落盘标准 JSON，保证互操作。
3. **加载器安装器统一产出 profile**：Fabric/Quilt 拉 JSON；Forge/NeoForge 跑 processors。
4. **候选 URL + 规则表的镜像**，不做镜像子类。
5. **两级缓存 + 原子写 + 流式哈希 + Range 续传**。
6. **规则引擎补全 os.arch / features / os.versionRange**。
7. **Java 选择用规则表**（版本 JSON + 版本段 + 平台约束），托管 JRE 优先。
8. **隔离 = run directory 解析函数**，不是布尔字段。
9. **固定日志契约 + 退出分类 + token 脱敏**。
10. **pkce + 设备码双流程；令牌进钥匙串**。

### 不抄

1. 私有 meta 作为唯一版本来源。
2. ForgeWrapper / 外部 installer 作为默认安装路径（只做兜底）。
3. 自研 DAG 任务引擎、core 绑 UI 属性。
4. patches 与扁平 JSON 双写。
5. 字符串拼命令行再切分。
6. 明文存 refresh token。
7. 把 OptiFine 的 jar patch 逻辑塞进 v1。
8. 为每个镜像/整合包格式写继承体系。
9. 依赖 WebView2/Tauri 换取 UI 开发速度（与轻量目标冲突）。
10. 复制 GPL/受限许可项目的代码（除非项目整体采用 GPLv3 并履行义务）。

## 8. 差异化定位

| 竞品强项 | nailong 的选择 |
|---|---|
| Prism 的模组生态与 meta 组件 | 不做内容平台；先把"装/启动"做稳，内容生态 v2 |
| HMCL 的功能全覆盖 | 不追求功能数量；v1 只做核心路径，减少认知负担 |
| PCL 的国内网络优化 | 学习其多源策略，但用 Rust 重新实现（候选/续传/并发） |
| Pandora 的精致 gpui UI | v1 用 egui 保证轻量与交付；UI 与 core 解耦，未来可换 |
| Quantum 的 iced 轻量 | 用 egui 获得更小的依赖面与更简单的自定义绘制 |

**一句话**：比 HMCL 轻、比 Prism 少依赖、比 PCL 跨平台，且启动正确性不打折。

## 9. 参考索引

### Prism

- 版本/组件：`launcher/minecraft/PackProfile.cpp`、`Component.cpp`、`VersionFile.cpp`、`LaunchProfile.cpp`
- 规则/库：`launcher/minecraft/Rule.h`、`Library.cpp`、`MojangVersionFormat.cpp`
- 下载/元数据：`launcher/net/*`、`launcher/meta/*`
- 启动：`launcher/LaunchController.cpp`、`launcher/launch/LaunchTask.cpp`、`launcher/launch/steps/*`
- 加载器：`launcher/modplatform/*`、`meta.prismlauncher.org`
- 账户：`launcher/minecraft/auth/*`

### HMCL

- 版本模型：`HMCLCore/.../game/GameInstanceManifest.java`、`GameInstancePatch.java`、`DefaultGameRepositorySnapshot.java`
- 启动：`HMCLCore/.../launch/DefaultLauncher.java`、`HMCL/.../game/LauncherHelper.java`
- 下载：`HMCLCore/.../task/FetchTask.java`、`download/DownloadProvider.java`、`HMCL/.../HMCLDownloadProvider.java`
- Forge：`HMCLCore/.../download/forge/ForgeNewInstallTask.java`
- 账户：`HMCLCore/.../auth/OAuth.java`、`auth/microsoft/MicrosoftService.java`
- Java：`HMCLCore/.../game/JavaVersionConstraint.java`、`download/java/mojang/MojangJavaDownloadTask.java`

### PCL

- 版本/资源：`Plain Craft Launcher 2/Modules/Minecraft/ModMinecraft.vb`
- 启动/登录：`.../ModLaunch.vb`
- 下载：`.../ModDownload.vb`、`Modules/Base/ModNet.vb`
- 模组加载器：`.../ModLoader.vb`
- 整合包：`.../ModModpack.vb`

### Rust 竞品

- Pandora：<https://github.com/Moulberry/PandoraLauncher>
- Quantum：<https://github.com/Mrmayman/quantumlauncher>
- Nitrolaunch：<https://github.com/Nitrolaunch/nitrolaunch>
- portablemc：<https://github.com/theorzr/portablemc>
- LightyLauncherLib：<https://github.com/Lighty-Launcher/LightyLauncherLib>
- OneLauncher：<https://github.com/Polyfrost/OneLauncher>
