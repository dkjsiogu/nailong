# 14 · 原型实现状态（0.1 → 0.4）

> 日期：2026-10-21
> 对应发布：GitHub Release `v0.4.3` 的 `nailong.exe`、`nailong_0.4.3_amd64.deb`
> 目的：把「设计稿」和「已经跑起来的代码」分开记账，避免把计划当成已完成。

## 1. 一句话结论

**垂直切片已经打通：Windows/Linux 上可以安装 Minecraft 1.21.1，并选择 Fabric / NeoForge / Forge / 原版，用离线账户启动到主菜单。**

0.4.2 把 Forge/NeoForge 的安装数据从“整个安装器”改成“按需读取核心条目”：首次安装只取约 3 MB，而不是 6–9 MB；不支持 Range 的镜像自动改用官方源或完整包兜底。

0.4.3 补上两项基础体验：加载器依赖的原版不再出现在主版本列表里（管理页仍可见、可删）；已完整安装的加载器再次点安装会直接复用，不再下载补丁或重跑 processor。3 MB 客户端补丁也改为 4 段并行读取。

发布形态刻意保持简单：Windows 只发一个 `nailong.exe`；Linux 只发一个可双击/`apt install` 的 `.deb`。

这不是 v1.0。它刻意只做一条路径，用来验证三件事：

1. 版本 JSON 的继承/规则/classpath 解析是对的；
2. 下载与安装能落盘、能重跑；
3. NeoForge 的 modern ModLauncher 启动方式能被正确复现。

## 2. 已完成（有代码 + 有实测）

| 能力 | 实现位置 | 状态 | 验证方式 |
|---|---|---|---|
| 版本清单 / 版本 JSON / `inheritsFrom` 合并 | `core::manifest` | 完成 | 1.21.1、NeoForge profile 实装解析 |
| 规则（OS/arch/features/versionRange） | `core::manifest` | 完成 | 单元测试（含 Windows 版本区间） |
| Maven 坐标、classifier、`@zip` 扩展 | `core::manifest` | 完成 | 单元测试 |
| 原版安装（client jar / libraries / assets / logging） | `core::install::vanilla` | 完成 | 空目录安装 1.21.1，3911 个资源对象 |
| 并行下载、sha1 校验、重试、BMCLAPI 重写 | `core::http` | 完成 | 实际安装；镜像参数可切换 |
| Java 扫描与版本选择 | `core::java` | 完成 | 识别 GraalVM 21 / OpenJDK 21 |
| natives 解压与去重 | `core::launch` | 完成 | 1.21.1 启动 |
| 启动参数拼接、日志落盘 | `core::launch` | 完成 | `--print-plan` + 实际启动 |
| 离线账户 UUID | `core::launch` | 完成 | 与 Java `nameUUIDFromBytes` 对齐，单元测试 |
| NeoForge 安装 | `core::install::neoforge` | 完成（精简数据 + 原生 processors） | 1.21.1 / 21.1.209 安装并启动到渲染 |
| NeoForge 启动 | `core::launch` | 完成 | 到主菜单；纹理图集加载完成 |
| Fabric 安装 / 启动 | `core::install::fabric` | 完成（原生 profile） | 1.21.1 / 0.19.5 安装并启动到渲染 |
| Forge 安装 / 启动 | `core::install::forge` | 完成（精简数据 + 原生 processors） | 1.21.1 / 52.1.20 安装并启动到渲染 |
| 四加载器选择 | `nailong-ui` | 完成 | 安装页按 Minecraft / Fabric / NeoForge / Forge / 原版排列 |
| Modrinth 整合包导入 | `core::install::modpack` | 完成 | `.mrpack` → 加载器 + 151 模组 + 独立 gameDir |
| Windows 便携目录 | `core::paths` | 完成 | 默认使用 exe 同目录 `.minecraft` |
| CLI（list/java/info/install/launch） | `nailong-cli` | 完成 | 打包产物冒烟 |
| GUI（版本下拉、安装进度、缺文件补全、启动、设置） | `nailong-ui` | 可用 | Xvfb 下走完安装 / 补全 / 启动；实际 NeoForge 到主菜单 |
| 版本管理 | `nailong-ui` | 完成 | 基础版本隐藏、版本/整合包删除、被依赖保护 |
| 重复安装快速路径 | `core::install::forge` / `neoforge` | 完成 | health 通过则 0 下载、0 processor |
| Windows 交叉编译 | `x86_64-pc-windows-gnu` | 通过 | 无 MinGW DLL 依赖，仅系统 DLL |
| 发布包 | GitHub Release `v0.4.3` | 完成（CI 构建） | 单 exe 约 12 MB / deb 约 4.3 MB |

### 2.1 实测记录

- **从零安装**：`nailong-cli --root /tmp/nailong-fresh install neoforge 1.21.1 21.1.209` 退出码 0。
- **启动**：同一目录 `launch neoforge-21.1.209 --username Steve --memory 4096`，日志出现
  `Setting user: Steve`、`Backend library: LWJGL version 3.3.3+5`、`Created: ... texture atlas`。
  无显示器环境用 Xvfb；音频设备缺失只产生 ALSA 警告，不影响进入游戏。
- **GUI**：打包后的 `nailong` 在 Xvfb 中持续运行；通过 xdotool 点击“启动”后游戏进程到达主菜单。
- **体积**：Linux GUI 11.6 MB / CLI 4.9 MB；Windows GUI 11.4 MB（CI 原生 MSVC 构建）/ CLI 4.8 MB（未压缩）。
- **内存**：Linux + Xvfb + llvmpipe 下 GUI 空闲 RSS 约 **135 MB**。这是软件渲染上界，未达到 120 MB 目标；Windows 真实 GPU 待测。

### 2.2 关键实现细节

- **classpath 特判**：NeoForge 的 `bootstraplauncher` / `modlauncher` 主类下，**不能**把原版 client jar 或 patched client jar 放进 classpath，否则会出现
  `Modules minecraft and _1._21._1 export package ...` 或 `more than one module named neoforge`。
  FML 会自己从 `libraries` 找到 patched client 与 universal jar。代码里对应
  `remove_classpath_args` 之后对 `resolved.client_jar` 的剔除。
- **离线 UUID**：`MD5("OfflinePlayer:" + name)`，再置 version=3、variant=2；与 Java 的
  `UUID.nameUUIDFromBytes` 一致。测试向量：`Notch -> b50ad385829d3141a2167e7d7539ba7f`。
- **版本列表**：主界面只展示用户安装的叶子版本；被加载器继承的原版标记为 `hidden`，管理页才列出。删除前检查“被多少版本依赖/被多少整合包使用”，避免把基础版本删掉。
- **processor 安装**：按需读取 Forge/NeoForge 安装器的核心条目（约 3 MB），
  精简缓存为 `*.slim.zip`；3 MB 补丁分 4 段并行读取；Rust 解析 `install_profile.json` 的 data/libraries/processors，
  只执行客户端的 processor（mappings、jarsplitter、renamer、binarypatcher 等），再写出
  `versions/<id>/<id>.json`。不执行官方 GUI installer，也不保留完整安装器。
- **下载器**：阻塞 `reqwest` + rustls(ring)，rayon 并发；单文件原子写；sha1 不匹配重试。
  没有 async 运行时，换取更小的依赖面与更简单的错误处理。

## 3. 与设计稿的偏差（必须如实披露）

| 设计稿 | 原型实际 | 影响 / 下一步 |
|---|---|---|
| Forge/NeoForge 原生 processors，外部 installer 仅兜底 | **Rust 原生驱动 processor 链 + 按需读取数据** | 只缓存约 3 MB 核心数据；不执行官方 GUI installer；需要完整包时自动回退 |
| 托管 JRE（Mojang runtime） | 只用系统 Java 21 | 用户需预装 Java；这是“开箱即用”目标的主要缺口 |
| 异步 runtime + 事件总线 + 可取消任务 | 阻塞调用 + 线程 + 进度回调；暂无取消 | 下载与安装已有阶段/字节进度；下一步做取消令牌 |
| 实例（Instance）抽象、配置存储 | 直接以版本目录为单位；无配置文件 | 目前只能切版本，不能多实例隔离 |
| 完整 GUI（路由/向导/设置/下载中心） | 单页、版本下拉、进度页、补全提示 | 核心路径清楚；下载中心/设置页未做 |
| 整合包 / 正版登录 / 自动更新 | 未实现 | 下一步做整合包与账户 |
| 微软正版登录 | 未实现（离线） | 按决定推迟到 M3.5 |
| 自动更新 / 签名 / 崩溃报告 | 未实现 | 发布前补齐 |
| 单元/集成测试体系 | **22 个测试** | 缺夹具、mock HTTP、启动 argv 快照、Windows CI |

## 4. 已知问题与风险

1. **Windows 只在 Linux 上交叉编译通过**，还没有 Windows 实机启动记录；OpenGL 驱动、路径、Java 发现都可能出问题。
2. **processor 安装**依赖 `install_profile` 结构和 processor 工具；上游改格式时需要跟进。精简读取还依赖官方 ZIP 结构；不支持 Range 或结构异常时会回退完整包。
3. **离线账户会触发 401**：游戏向 Mojang 请求 user properties 被拒，日志有 `InvalidCredentialsException`，但不阻塞单人游戏；后续可关闭该请求或接正版登录。
4. **安装没有取消按钮**；关闭窗口可中断，重新打开会检测并补全。
5. **GUI 内存 135 MB**（软件渲染）高于 120 MB 目标；需要真实 GPU 复测，或减少字体/纹理占用。
6. **无断点续传**：普通文件是“已存在且哈希正确则跳过”；安装器核心数据用 HTTP Range 只取所需条目，但不是断点续传。中断后重跑会重新读取约 3 MB。
7. **无安装锁**：并发启动两个安装可能写同一目录；原型未处理。
8. **资产下载没有虚拟资源映射**（legacy virtual assets）验证；1.21.1 不需要，旧版本可能需要。

## 5. 下一步（按优先级）

1. **Windows 实机验证**：在一台 Windows 10/11 + Java 21 上跑安装与启动，记录失败点。
2. **安装取消**：增加取消令牌，让用户在下载/安装中安全停止。
3. **processor 校验增强**：outputs SHA1 校验、取消令牌、失败断点续跑。
5. **测试与 CI**：版本 JSON 夹具、规则/合并/argv 快照、mock HTTP（重试/哈希失败）、`fmt/clippy/test`。
6. **托管 JRE**：至少做“检测缺失 → 引导安装/下载 Temurin”的闭环。
7. **实例与配置**：TOML 配置、每实例 game dir/Java/内存，脱离“版本即实例”。
8. **发布工程**：版本号、CHANGELOG、SHA256、签名、自动更新。

## 6. 复现命令

```bash
# 构建
cargo build --release -p nailong-ui -p nailong-cli
cargo build --release --target x86_64-pc-windows-gnu -p nailong-ui -p nailong-cli

# 测试
cargo test --workspace
cargo clippy --all-targets

# 空目录安装 + 启动（Linux，需 Java 21）
./target/release/nailong --root /tmp/nailong-fresh install neoforge 1.21.1 21.1.209
# 首次安装应只生成 cache/nailong/installers/*.slim.zip（约 3 MB），不生成完整 installer
xvfb-run -a ./target/release/nailong --root /tmp/nailong-fresh \
  launch neoforge-21.1.209 --username Steve --memory 4096
```

> 注：CLI 包名是 `nailong-cli`，二进制名是 `nailong`；GUI 包名是 `nailong-ui`，打包时重命名为 `nailong`。构建产物路径以 `target/release/` 为准。
