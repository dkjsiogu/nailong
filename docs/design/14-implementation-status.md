# 14 · 原型实现状态（0.1.0）

> 日期：2026-10-20
> 对应发布：GitHub Release `v0.1.0` 的 `nailong.exe`、`nailong_0.1.0_amd64.deb`
> 目的：把「设计稿」和「已经跑起来的代码」分开记账，避免把计划当成已完成。

## 1. 一句话结论

**垂直切片已经打通：Windows/Linux 上可以安装 Minecraft 1.21.1 + NeoForge 21.1.209，并用离线账户启动到主菜单。**

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
| NeoForge 安装 | `core::install::neoforge` | **可用，但复用官方 installer** | 1.21.1 / 21.1.209 空目录安装成功 |
| NeoForge 启动 | `core::launch` | 完成 | 到主菜单；纹理图集加载完成 |
| CLI（list/java/info/install/launch） | `nailong-cli` | 完成 | 打包产物冒烟 |
| GUI（实例列表、用户名/内存、安装、启动、日志） | `nailong-ui` | 可用原型 | Xvfb 下启动；xdotool 点击启动成功 |
| Windows 交叉编译 | `x86_64-pc-windows-gnu` | 通过 | 无 MinGW DLL 依赖，仅系统 DLL |
| 发布包 | `dist/` | 完成 | 6.9 MB zip / 7.2 MB tar.gz |

### 2.1 实测记录

- **从零安装**：`nailong-cli --root /tmp/nailong-fresh install neoforge 1.21.1 21.1.209` 退出码 0。
- **启动**：同一目录 `launch neoforge-21.1.209 --username Steve --memory 4096`，日志出现
  `Setting user: Steve`、`Backend library: LWJGL version 3.3.3+5`、`Created: ... texture atlas`。
  无显示器环境用 Xvfb；音频设备缺失只产生 ALSA 警告，不影响进入游戏。
- **GUI**：打包后的 `nailong` 在 Xvfb 中持续运行；通过 xdotool 点击“启动”后游戏进程到达主菜单。
- **体积**：Linux GUI 11.6 MB / CLI 4.9 MB；Windows GUI 10.5 MB / CLI 4.8 MB（未压缩）。
- **内存**：Linux + Xvfb + llvmpipe 下 GUI 空闲 RSS 约 **135 MB**。这是软件渲染上界，未达到 120 MB 目标；Windows 真实 GPU 待测。

### 2.2 关键实现细节

- **classpath 特判**：NeoForge 的 `bootstraplauncher` / `modlauncher` 主类下，**不能**把原版 client jar 或 patched client jar 放进 classpath，否则会出现
  `Modules minecraft and _1._21._1 export package ...` 或 `more than one module named neoforge`。
  FML 会自己从 `libraries` 找到 patched client 与 universal jar。代码里对应
  `remove_classpath_args` 之后对 `resolved.client_jar` 的剔除。
- **离线 UUID**：`MD5("OfflinePlayer:" + name)`，再置 version=3、variant=2；与 Java 的
  `UUID.nameUUIDFromBytes` 一致。测试向量：`Notch -> b50ad385829d3141a2167e7d7539ba7f`。
- **安装器调用**：下载 `neoforge-<ver>-installer.jar`，执行
  `java -jar ... --installClient <root>`，然后读取生成的
  `versions/neoforge-<ver>/neoforge-<ver>.json`。启动器运行时不依赖该 jar。
- **下载器**：阻塞 `reqwest` + rustls(ring)，rayon 并发；单文件原子写；sha1 不匹配重试。
  没有 async 运行时，换取更小的依赖面与更简单的错误处理。

## 3. 与设计稿的偏差（必须如实披露）

| 设计稿 | 原型实际 | 影响 / 下一步 |
|---|---|---|
| Forge/NeoForge 原生 processors，外部 installer 仅兜底 | **直接调用官方 installer** | 安装阶段需要系统 Java、进度不可控、镜像不可控；下一步实现 processors 并保留兜底 |
| 托管 JRE（Mojang runtime） | 只用系统 Java 21 | 用户需预装 Java；这是“开箱即用”目标的主要缺口 |
| 异步 runtime + 事件总线 + 可取消任务 | 阻塞调用 + 线程；GUI 只显示状态文字 | 安装时 UI 会等待；下一步做进度事件与取消 |
| 实例（Instance）抽象、配置存储 | 直接以版本目录为单位；无配置文件 | 目前只能切版本，不能多实例隔离 |
| 完整 GUI（路由/向导/设置/下载中心） | 单页最小界面 | 只够验证启动链路 |
| Fabric / Forge / 整合包 | 未实现 | 下一步先 Fabric，再 Forge |
| 微软正版登录 | 未实现（离线） | 按决定推迟到 M3.5 |
| 自动更新 / 签名 / 崩溃报告 | 未实现 | 发布前补齐 |
| 单元/集成测试体系 | **9 个核心单元测试** | 缺夹具、mock HTTP、启动 argv 快照、CI |

## 4. 已知问题与风险

1. **Windows 只在 Linux 上交叉编译通过**，还没有 Windows 实机启动记录；OpenGL 驱动、路径、Java 发现都可能出问题。
2. **安装器路径**依赖官方 installer 的 `--installClient` 行为；NeoForge 改参数或需要网络访问时可能失败。
3. **离线账户会触发 401**：游戏向 Mojang 请求 user properties 被拒，日志有 `InvalidCredentialsException`，但不阻塞单人游戏；后续可关闭该请求或接正版登录。
4. **GUI 安装期间无进度条**，大文件下载时看起来“卡住”；CLI 会打印阶段。
5. **GUI 内存 135 MB**（软件渲染）高于 120 MB 目标；需要真实 GPU 复测，或减少字体/纹理占用。
6. **无断点续传**：当前是“已存在且哈希正确则跳过”，不是 HTTP Range 续传；中断后重跑会重下未完成文件。
7. **无安装锁**：并发启动两个安装可能写同一目录；原型未处理。
8. **资产下载没有虚拟资源映射**（legacy virtual assets）验证；1.21.1 不需要，旧版本可能需要。

## 5. 下一步（按优先级）

1. **Windows 实机验证**：在一台 Windows 10/11 + Java 21 上跑安装与启动，记录失败点。
2. **GUI 进度与取消**：把 core 的下载/安装回调接到 UI，至少显示“当前文件/总数/字节”。
3. **NeoForge 原生安装**：实现 processors（downloads、srg 合并、patch、outputs 校验），官方 installer 保留为开关。
4. **Fabric 支持**：profile JSON + 库下载，作为第二个加载器验证抽象。
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
xvfb-run -a ./target/release/nailong --root /tmp/nailong-fresh \
  launch neoforge-21.1.209 --username Steve --memory 4096
```

> 注：CLI 包名是 `nailong-cli`，二进制名是 `nailong`；GUI 包名是 `nailong-ui`，打包时重命名为 `nailong`。构建产物路径以 `target/release/` 为准。
