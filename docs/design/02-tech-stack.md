# 02 · 技术选型

> 选型原则：**能用标准库就不引依赖；能被 UI 替换的绝不进核心；每个依赖都要解释"为什么是它"。**
> 文中的体积/内存为量级估计，M0 会用最小可运行程序实测并回填（见 §11）。

## 1. 语言与工具链

| 项 | 选择 | 理由 |
|---|---|---|
| 语言 | Rust 2024 edition | 内存安全、单二进制、跨平台、生态成熟 |
| MSRV | **1.95** | eframe/egui 0.37 要求 1.95；CI 同时测 stable |
| 构建 | Cargo workspace，resolver 3 | 统一依赖版本 |
| 格式化 | `rustfmt` | 统一风格 |
| Lint | `clippy -D warnings` | 把常见错误挡在 CI |
| 依赖审计 | `cargo-deny` + `cargo-audit` | 许可证/漏洞/重复依赖 |
| 未使用依赖 | `cargo-machete` | 控制依赖膨胀 |
| 测试 | 内置 + `insta` 快照 + `wiremock` | 纯函数与网络行为都可测 |

> 当前开发机 stable 为 1.93，需 `rustup update stable`；仓库提供 `rust-toolchain.toml` 固定 `1.95` 起步，CI 用最新 stable 验证。

## 2. GUI 选型

### 2.1 候选对比（本机实测 + 发布物对照）

同一台 Linux x86_64 机器、Rust 1.99.1、同一 release 配置（`opt-level="z"`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`）构建"显示一行中英文"的最小窗口，在 Xvfb 1280×800 + Mesa llvmpipe（软件渲染）下测量。**绝对值被软件渲染放大，横向排序与量级仍有参考价值；Windows 有 GPU 时内存通常更低。**

| 方案 | 渲染 | 二进制 | 启动到窗口 | 静置 RSS | 中文/IME | 结论 |
|---|---|---:|---:|---:|---|---|
| **egui 0.37 + eframe(glow)** | OpenGL | **5.15 MiB** | **36–40 ms** | 107 MiB（软件渲染；Windows 经验 40–80 MiB） | 0.37 系统字体 + IME 改进；Windows 有 winit 光标 bug | **首选** |
| **iced 0.13 + tiny-skia**（0.14 更好） | CPU | **3.85 MiB** | **21–22 ms** | **30.7 MiB** | cosmic-text 系统字体；0.14 加入 IME | **备选 #1（最强轻量数据）** |
| iced + wgpu | GPU | 6.02 MiB | 98–115 ms | 217 MiB（软件 Vulkan 放大） | 同上 | 不选（wgpu 在软件渲染下过重） |
| Slint 1.18 + femtovg | GPU | 9.72 MiB | 88–101 ms | 129 MiB | 需显式字体；CJK 需打包 | 备选 #2（GPL 版可用） |
| Tauri / wry | WebView2 | 安装包 8–25 MiB + WebView2 | 0.3–2 s | 100–300 MB | Web 原生，最稳 | 不选（违背轻量） |
| gpui | GPU | Pandora 便携包 53.4 MB | 100–300 ms | 80–200 MB | 依赖平台实现 | 不选（API 不稳/体积大） |

**Windows 真实发布物对照**：

| 应用 | GUI | Windows 资产 |
|---|---|---|
| PCL 2.99 | WPF / .NET Framework 4.8 | **5.70 MiB 单 exe** |
| QuantumLauncher | iced | 8.1 MiB zip |
| LiquidLauncher | Tauri | 8.0 MiB setup |
| OneLauncher | Freya/Skia | 19.7 MiB exe |
| Modrinth App | Tauri | 25.1 MiB setup |
| Pandora | gpui | 53.4 MiB 便携包 |

> 数据来源：本次研究中对各框架最小程序的实际构建/测量，以及公开 release 资产大小。完整方法与局限见 [`docs/bench/gui-2026-10.md`](../bench/gui-2026-10.md)；M0 会在 Windows 上复测。
> 注：上线构建还会加入 `system_fonts`、`accesskit`、`wayland`、reqwest、zip、keyring 等，预计比 hello 增加 5–7 MB；目标 15 MB 仍有余量，但需要 CI 持续盯住。

### 2.2 为什么是 egui

1. **纯 Rust、无系统 WebView**：不依赖 WebView2/GTK WebKit，符合"单文件、无运行时"。
2. **实测够轻**：最小窗口 5.15 MiB / 36–40 ms；加入业务、图标、字体子集后预计 8–12 MB，满足 ≤15 MB。
3. **依赖面可控**：用 `glow`（OpenGL）而非 `wgpu` 可显著减小体积与编译时间；老机器可用软件 GL 回退（视驱动）。
4. **立即模式适合启动器**：界面是"状态快照 → 绘制"，与我们的 `Event + UiState` 架构天然匹配；不需要复杂的 retained 组件树。
5. **自定义绘制强**：卡片、圆角、渐变、进度环、动画都能自绘，容易做出统一视觉。
6. **系统字体**：eframe 0.37 的 `system_fonts` 特性可直接加载系统字体，避免把整套 CJK 字体打进二进制。
7. **可替换**：core 不依赖 egui；若后续视觉/无障碍不达标，可迁移到 iced/slint/gpui。

**为什么不直接选更轻的 iced？** iced+tiny-skia 的实测数据更好（3.85 MiB / 21 ms / 30.7 MiB），但：
- egui 的即时模式对"多页面 + 大量自定义控件 + 快速迭代"更省人力；本项目是单人/小团队。
- egui 的 Windows 实际内存预期 40–80 MB，仍在 120 MB 预算内；软件渲染下 107 MiB 是上界。
- iced 的 Elm 架构会带来大量 state/message 样板，且自定义绘制不如 egui 直接。
- 如果 M0/M2 实测 egui 的空闲 RSS 或耗电不达标，按 ADR-001 的触发条件迁移到 iced+tiny-skia（core 不变）。

**测量环境警告**：上表是 Linux + Xvfb + llvmpipe 的结果，软件渲染会放大 OpenGL/wgpu 内存；Windows 真实 GPU 下 egui 通常 40–80 MB。最终以 M0 的 Windows 基准为准。

### 2.3 egui 的风险与缓解

| 风险 | 缓解 |
|---|---|
| 中文 IME（候选窗位置、组合输入） | M0 在 Windows/macOS/Linux 实测；必要时接入 `egui-winit` 的 IME 事件修补；备选 iced |
| 无障碍（屏幕阅读器） | 启用 `accesskit`；v1 以键鼠可用为主，文档说明限制 |
| 文本排版（长段落、换行、选取） | 统一用 `egui::Label` + 自定义 `LayoutJob`；避免超长文本；日志用等宽 |
| 无原生菜单栏 | 用侧边栏/命令面板；文件对话框用 `rfd` 原生 |
| 高 DPI/多显示器 | 使用 `pixels_per_point` 与 `ViewportCommand::InnerSize`；CI 截图回归 |
| 大列表性能 | `ScrollArea::show_rows` 虚拟化；图标懒加载 + LRU |
| 空闲重绘耗电 | 默认不连续重绘；用 `request_repaint_after` 降频，动画结束即停 |
| 系统无中文字体（精简 Linux） | 内置 Noto Sans SC 子集（约 2–3 MB）作为回退；仅缺字时使用 |

### 2.4 其他方案为何不选

- **Tauri**：WebView2 在 Windows 10/11 通常已有，但它是独立进程组，空闲内存与启动时间显著高于原生；"无运行时依赖"不成立。前端生态的优势对只有 6–8 个页面的启动器收益有限。
- **Slint**：运行时与体积优秀，但 GPL/商业双许可；如果项目最终选 MIT/Apache，则不能使用 GPL 版本。作为备选需先定许可证。
- **gpui**：Pandora 证明可行，但 API 迭代快、文档少、编译重；对单人/小团队风险高。保留为 v2 视觉升级选项。
- **iced**：实测最轻（3.85 MiB / 21 ms / 30.7 MiB），且 tiny-skia 是 CPU 渲染、无 GPU 驱动依赖；代价是 Elm 架构样板与自定义控件成本。若 egui 的内存/IME/无障碍验收失败，迁移到 iced 0.14 + tiny-skia。

## 3. 异步与网络

| 组件 | 选择 | 理由 |
|---|---|---|
| 运行时 | **tokio 1.54**（rt-multi-thread, fs, process, sync, time） | 生态标准；子进程/文件/定时器齐全 |
| HTTP | **reqwest 0.13** + `rustls` | 连接池、HTTP/2、流式下载、重定向控制；rustls 避免 OpenSSL 运行时依赖 |
| TLS 根证书 | reqwest 0.13 的 `rustls` 特性默认使用 `rustls-platform-verifier`（Windows CryptoAPI / macOS Security / Linux 系统根） | 企业代理/自签根证书可用；不提供"关闭校验"开关 |
| JSON | `serde_json` | 标准；保留未知字段 |
| TOML | `toml` 1.x | 人类可读配置 |
| 并发 | `tokio::sync::Semaphore` + `JoinSet` | 全局/单主机限流、结构化并发 |
| 取消 | `tokio_util::sync::CancellationToken` | 组合式取消，子任务自动感知 |
| 哈希 | `sha1` / `sha2` / `md-5`（RustCrypto） | 流式 `Digest`，下载时同步计算 |
| 压缩 | `zip` 9（默认只开 deflate，其他按需） | 成熟；解压走 `spawn_blocking` |
| LZMA | `lzma-rs` | Mojang JRE 的 `.lzma`；纯 Rust，避免 C 依赖 |
| 锁 | `fs4` | 跨平台 advisory file lock |
| 日志 | `tracing` + `tracing-subscriber` + `tracing-appender` | 结构化、异步、按文件轮转 |
| 错误 | `thiserror` | 类型化错误 + 稳定错误码；UI 不解析字符串 |

> 不选 `ureq`（阻塞，需要额外线程池）、不选 `hyper` 直接用（样板多）、不选自建 TLS。

## 4. 平台集成

| 功能 | crate | 说明 |
|---|---|---|
| 原生文件对话框 | `rfd` 0.17 | Windows COM / macOS / Linux portal；有异步 API |
| 打开文件夹/URL | `open` 5 | 调系统默认程序 |
| 系统信息 | `sysinfo` 0.39 / `os_info` | CPU/内存/OS 版本（规则引擎用 Windows build） |
| Windows 注册表 | `winreg` 0.56 | Java 探测；仅 `cfg(windows)` |
| 钥匙串 | `keyring` 4 | Credential Manager / Keychain / Secret Service |
| 单实例 | `interprocess` 2.4（本地 socket） | 第二实例把 CLI 参数/深链交给主实例后退出；比命名互斥更可靠 |
| 图标/字体 | `image`（png/jpeg）+ `fontdb`（可选） | 头像、实例图标；系统字体加载 |
| 系统托盘（v1.1） | `tray-icon` | 关闭到托盘；v1 不做 |
| 自动更新（v1.1） | 自研 + `self_update` 参考 | 签名校验，不直接信任 GitHub |

## 5. 核心依赖清单（Cargo 草案）

```toml
[workspace]
resolver = "3"
members = ["crates/nailong-core", "crates/nailong-ui", "crates/nailong-cli"]

[workspace.package]
edition = "2024"
rust-version = "1.95"
version = "0.1.0"
license = "GPL-3.0-or-later"

[workspace.dependencies]
# async / net
tokio = { version = "1.54", features = ["rt-multi-thread", "macros", "fs", "process", "io-util", "sync", "time"] }
tokio-util = { version = "0.7", features = ["rt"] }
reqwest = { version = "0.13", default-features = false, features = ["rustls", "json", "stream", "gzip", "brotli", "http2", "system-proxy"] }
futures = "0.3"

# data
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "1"
url = "2"
uuid = { version = "1", features = ["v4", "v5", "serde"] }
base64 = "0.23"
rand = "0.10"

# files / compression / hash
zip = { version = "9", default-features = false, features = ["deflate"] }
lzma-rs = "0.3"
sha1 = "0.11"
sha2 = "0.11"
md-5 = "0.11"
fs4 = { version = "1", features = ["tokio"] }
directories = "6"
tempfile = "3"

# errors / logging
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-appender = "0.2"

# platform
keyring = "4"   # 默认启用 apple/windows/zbus-secret-service 平台存储
rfd = "0.17"
open = "5"
interprocess = "2.4"
sysinfo = "0.39"
image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }

[target.'cfg(windows)'.dependencies]
winreg = "0.56"

# UI（仅 nailong-ui）
eframe = { version = "0.37", default-features = false, features = ["glow", "default_fonts", "system_fonts", "x11", "wayland", "accesskit"] }
egui_extras = { version = "0.37", features = ["image"] }
rust-i18n = "4"

# CLI
clap = { version = "4", features = ["derive"] }

# dev
insta = "1"
wiremock = "0.6"
```

> 版本以 2026-10 crates.io 为准；实现时用 `cargo add` 并提交 `Cargo.lock`。上表是"意图清单"，不是最终 lockfile。
> `eframe` 的默认特性包含 `wgpu`；为了体积我们显式关闭默认特性并选 `glow`。若某些机器 OpenGL 不可用，再提供 `wgpu` 构建变体。

## 6. 不引入的依赖

| 依赖 | 原因 |
|---|---|
| `anyhow`（core 内） | 丢失错误类型；只允许在 CLI/UI 顶层 |
| `serde_yaml` | YAML 解析歧义/安全问题；配置用 TOML |
| `sqlx` / `rusqlite` | v1 无数据库需求；文件足够 |
| `tauri` / `wry` | WebView 与轻量目标冲突 |
| `qt` 绑定 / `gtk` 直接绑定 | 体积与跨平台打包成本 |
| `openssl` | 动态库依赖；rustls 足够 |
| `rayon`（全局） | 与 tokio blocking 池重复；局部必要时再用 |
| `regex`（热路径） | 体积与回溯风险；用简单解析/`memchr` |
| `chrono` / `time` | 若只需 RFC3339，可用 `jiff` 或 `time`；避免两个时间库并存（实现时二选一） |

## 7. 许可证兼容性

| 依赖 | 许可证 | 兼容 GPLv3 | 兼容 MIT/Apache |
|---|---|---|---|
| tokio / reqwest / serde / egui | MIT / Apache-2.0 | ✅ | ✅ |
| zip | MIT | ✅ | ✅ |
| keyring | MIT / Apache-2.0 | ✅ | ✅ |
| lzma-rs | MIT | ✅ | ✅ |
| Slint | GPL-3 / 商业 | ✅（GPL） | ❌ |
| 字体（Noto Sans SC） | OFL-1.1 | ✅ | ✅（保留版权声明） |
| 图标（Phosphor） | MIT | ✅ | ✅ |

- 引入任何 GPL/AGPL/SSPL 依赖前必须评审；`cargo-deny` 配置 allowlist。
- 如果最终选择 MIT/Apache，Slint 不能作为实现；egui/iced 均可。

## 8. 构建与分发

### 8.1 目标三元组

| 平台 | target | 备注 |
|---|---|---|
| Windows x64 | `x86_64-pc-windows-msvc` | 静态 CRT（`+crt-static`）减少 VC 运行库依赖 |
| Windows arm64 | `aarch64-pc-windows-msvc` | v1.1 |
| Linux x64 | `x86_64-unknown-linux-gnu` | 在 Ubuntu 22.04 构建以兼容较老 glibc |
| Linux arm64 | `aarch64-unknown-linux-gnu` | v1.1 |
| macOS x64/arm64 | `x86_64-apple-darwin` / `aarch64-apple-darwin` | v1.1；`.app` + ad-hoc 签名 |

### 8.2 发布形态

| 平台 | v1 | 后续 |
|---|---|---|
| Windows | 便携 zip（单 exe）+ Inno Setup 安装包 | MSIX、自动更新 |
| Linux | tar.gz + `.desktop` + 图标 | AppImage、deb、AUR |
| macOS | `.app`（未公证，右键打开） | 公证、DMG |

### 8.3 体积优化

```toml
[profile.release]
opt-level = "s"
lto = "thin"
codegen-units = 1
strip = "symbols"
panic = "abort"      # 先测量；需要 catch_unwind 时改回 unwind
```

- Windows 加 `-C target-feature=+crt-static`（需验证所有依赖兼容）。
- 不为单个功能引入大 crate；`image` 只开 png/jpeg。
- 嵌入资源压缩；字体用系统 + 子集回退。
- 不用 UPX（杀软误报、启动解压开销）。

### 8.4 CI 门禁

1. `cargo fmt --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo test`（三平台）
4. `cargo deny check`
5. `cargo machete`
6. 体积检查：Windows release exe ≤ 15 MB，否则失败并输出最大依赖贡献（`cargo bloat`）。
7. 启动基准：首帧时间与空闲 RSS 记录到趋势文件（允许 ±20% 波动）。

## 9. 为什么不用 Tauri（ADR-001 摘要）

- **依赖**：Windows 需要 WebView2；虽然 Win10/11 常见，但精简系统/企业镜像可能缺失，需要引导安装。
- **内存**：WebView2 多进程，空闲 150 MB+；与"<120 MB"目标冲突。
- **启动**：WebView 初始化 + 前端 bundle 解析，通常 0.5 s 以上。
- **安全面**：IPC、前端依赖链、CSP 都需要额外维护。
- **收益**：Web 前端开发快、生态大；但本项目页面数量少、视觉自定义强，收益不足以抵消成本。

## 10. 为什么不用 Slint（ADR-002 摘要）

- 体积/启动优秀，声明式开发效率高。
- **许可证**：GPL-3 或商业授权；若项目要 MIT/Apache，则不可用。
- 生态与自定义绘制能力不如 egui 直接；中文排版需验证。
- 结论：许可证确定后若为 GPLv3 且 egui 验收失败，可评估 Slint。

## 11. 待实测（M0 基准）

| 指标 | 方法 | 初始基线（Linux/llvmpipe） | 目标（Windows） |
|---|---|---|---|
| exe 大小 | 最小 eframe 窗口 + 核心依赖，release + strip | 5.15 MiB（空窗口） | ≤ 15 MB |
| 空闲 RSS | 空窗口 + 20 个假实例，静置 60 s | 107 MiB（软件渲染上界） | ≤ 120 MB |
| 首帧 | `main` 打点 → `update` 第一帧 | 36–40 ms | ≤ 400 ms |
| 中文字体 | Windows/macOS/精简 Linux 三环境 | 无豆腐块 |
| IME | 微软拼音/搜狗输入中文实例名 | 候选窗正常、无重复字符 |
| 下载时帧率 | 模拟 64 并发进度事件 | 60 FPS |

结果回填到本文与 [10 路线图](10-roadmap.md)。

## 12. 决策记录

| ID | 决策 | 备选 | 理由 | 回滚条件 |
|---|---|---|---|---|
| ADR-001 | egui/eframe | iced、Slint、gpui、Tauri | 5.15 MiB/40 ms，自绘强、开发快；Windows 内存预计 40–80 MB | RSS >120 MB 或 IME/无障碍验收失败 → iced+tiny-skia |
| ADR-002 | glow 渲染 | wgpu | 体积小、依赖少 | 老 GPU 兼容问题 → 提供 wgpu 变体 |
| ADR-003 | tokio | async-std、smol | 生态、子进程/文件支持 | 无需回滚 |
| ADR-004 | rustls + 平台证书校验 | native-tls | 无 OpenSSL 依赖；`rustls-platform-verifier` 兼容企业代理 | 平台校验器不可用 → 内置 webpki 根 + 手动导入 |
| ADR-005 | 单 core crate | 多 crate | 编译快、样板少 | >2 万行或编译 >60 s → 拆分 |
| ADR-006 | 不做通用任务 DAG | 自研 Task 框架 | 可读、可测、无动态图 | 出现真正的动态依赖需求 → 重新评估 |
| ADR-007 | egui 系统字体 + 子集回退 | 内置完整 CJK | 体积 | 缺失字体环境 → 增大子集 |
