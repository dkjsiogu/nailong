# PCL 架构研究与 Rust 技术选型：面向 nailong（轻量 Minecraft 启动器）的设计输入

**研究对象**

- PCL：`/tmp/ref/pcl`，浅克隆，HEAD `6b9a50d`，`Plain Craft Launcher 2` 版本 **2.99.0.0**（VersionCode 413，正式版），2026-10-20 构建。**VB.NET + WPF，目标 .NET Framework 4.8**；`PCLCS` 与 `MeloongCore` 是 C# 辅助工程（`MeloongCore` 为 git submodule，本克隆为空）。
- 仓库内 `最新正式版.zip` 中的官方发布物：`Plain Craft Launcher 2.exe` = **5,980,160 B（5.70 MiB）**，zip = 2,664,629 B（2.54 MiB）。这是本文最重要的“轻量”基线数据。
- 代码规模：113 个 `.vb` 共 **42,631 行**，63 个 `.xaml` 共 **5,615 行**；`Controls/` 42 个文件，`Pages/` 66 个文件。

**方法**

- 只读源码（未修改 `/tmp/ref/pcl`），按调用链逐个函数阅读；行号均指 `Plain Craft Launcher 2/` 下的相对路径。
- Rust GUI 对比使用本机实际构建的 hello-world 程序（Linux x86_64，Rust 1.99.1，release + `opt-level="z"` + `lto="fat"` + `codegen-units=1` + `panic="abort"` + `strip=true`），在 Xvfb（1280×800，Mesa llvmpipe 软件渲染）下测量“进程启动→窗口可被 `xdotool` 搜索到”的耗时与 RSS。**这不是 Windows 数据**，软件渲染会显著抬高内存；绝对值应看作上界，横向排序与量级仍有参考价值。凡属实测，文中标“实测”；凡属发布物，标“发布物”；其余为源码推断或公开资料。

---

## 0. 给决策者的 10 条结论

1. **PCL 的“轻”不是 .NET 单文件发布的功劳，而是“框架依赖 + 把 DLL 当资源塞进 exe + 便携目录”的组合**：目标 .NET Framework 4.8（Win10/11 自带），exe 仅 5.7 MiB，依赖程序集以 `EmbeddedResource` 存放、运行时 `AssemblyResolve` 动态加载（`Application.xaml.vb:217-233`）。它没有自带运行时，所以才能这么小。
2. **PCL 真正的架构骨架是“加载器树”，不是页面**：`LoaderTask / LoaderCombo / LoaderDownload` 组成可嵌套、可取消、带 `ProgressWeight` 的任务树，启动流水线、下载、版本扫描全部复用它。Rust 版最值得抄的是这套“计划树 + 权重进度 + 取消传播”，而不是它的 UI。
3. **PCL 的版本模型是“官方 version.json 优先 + inheritsFrom 递归合并 + 兼容 HMCL patches”**；它不建私有 meta 服务（与 Prism 相反），也不把 loader 安装外包给外部 wrapper（与 Prism 的 ForgeWrapper 相反）。
4. **PCL 的规则引擎是“Windows 特化”的简化实现**：只认 `os.name=windows`（`os.version` 只对 Windows 做正则匹配，其它 OS 一律 false），`features` 里 demo/quick_play 硬编码（`ModMinecraft.vb:1554-1601`）。这在 Windows 上够用，但**不可移植**；Rust 版必须实现完整 rules。
5. **下载是 PCL 最“工程化”的部分**：单文件多线程分段（256 KiB 粒度、按最大剩余块 40% 切分）、按下载源隔离故障、失败源自动禁用、无 Range 支持时整体回退单线程重试一次、按连接均值自适应超时、低速 5 s 断链、跨 MC 文件夹按大小+SHA1 找同文件直接复制。默认 **64 线程**（设置值 63 + 1）。
6. **国内适配是“默认官方优先、慢时换镜像 + 按域名重写 + 每次请求多源回退”**：BMCLAPI 覆盖 assets/libraries/meta/Java/OptiFine/authlib-injector；Modrinth/CurseForge 走 `mod.mcimirror.top`；社区内容走 `gh-proxy.com`。默认“优先官方源，加载缓慢时换镜像”，而不是无脑镜像。
7. **PCL 的登录是标准 device code 六步链路**，实现干净，错误码映射到位；但 **token 明文存 ini**（`CacheMsV2*` / `LoginMsJson`），没有系统密钥库。Rust 版应改用 `keyring`，这是明确可超越点。
8. **Rust 侧没有“成熟到可以闭眼用”的 MC 启动器框架**：`minecraft-auth` 0.1.1、`mc-launcher-core` 0.1.2、`lighty-launcher` 26.x 都很年轻；生产项目（Modrinth、Pandora、LiquidLauncher）都是自己写 core。**认证与启动核心应自研，按 crate 只选基础设施。**
9. **GUI 实测排序（Linux/软件渲染，同一 hello-world）**：iced+tiny-skia（3.85 MiB / 21 ms / 30 MB）< egui（5.15 MiB / 36 ms / 71–107 MB）< slint（9.72 MiB / 95 ms / 86–129 MB）< iced+wgpu（6.02 MiB / 105 ms / 155–217 MB）。**如果“轻量”是硬指标，iced+tiny-skia 的数据最好；如果“开发效率 + 自定义绘制 + 生态”优先，egui 更合适。** Tauri/gpui 在这一目标下不推荐（见 B2/B6）。
10. **对 nailong 的推荐**：主选 **egui/eframe（glow，必要时 wgpu 回退）**，备选 **iced 0.14 + tiny-skia**；两者都能满足“单 exe ≤ 15 MB、冷启动 ≤ 400 ms”。但实测显示 iced+tiny-skia 在体积/内存/启动三项都更优（3.85 MiB / 30 MB / 21 ms），所以建议在 M0 做一次 **Windows 实机 spike 双跑**：若 egui 在“20 实例 + 图片”场景 RSS >100 MB 或空闲 CPU >1%，就切 iced。明确 **不做 WebView**。

---

# A. PCL 研究

## A1. 目录、模块划分与导航

### A1.1 物理结构

```
Plain Craft Launcher 2/
├─ Application.xaml(.vb)        应用入口：单例、命令行(--update/--gpu/--memory)、权限、日志、异常兜底
├─ FormMain.xaml(.vb)           主窗口：标题栏、五个顶级页、页面栈、动画、全局快捷键
├─ Modules/
│  ├─ Base/                     ModBase(路径/ini/进程/权限)、ModLoader(加载器树)、ModNet(HTTP+下载引擎)、
│  │                            ModAnimation(动画)、ModValidate(输入校验)、PclLogger(日志)、MyBitmap
│  ├─ Minecraft/                ModMinecraft(实例/JSON/库/资源)、ModLaunch(登录+参数+进程)、ModDownload(源与安装)、
│  │                            ModJava(Java 发现/下载)、ModModpack(整合包)、ModCrash(崩溃分析)、ModWatcher(进程/日志监视)
│  ├─ Resource/                 社区资源模型与搜索（Modrinth/CurseForge 抽象）
│  ├─ ModMain.vb                提示(Hint)、弹窗、下载任务 UI、杂项
│  ├─ ModEvent.vb               自定义事件（URL scheme / 命令行事件）
│  └─ ModSecret.vb              客户端 ID、UA/Referer 签名、主题色、更新（开源版多为 stub）
├─ Pages/                       页面：PageLaunch / PageDownload / PageInstance / PageSetup / PageOther / PageSelect / PageSpeed
├─ Controls/                    自绘 WPF 控件库：MyButton/MyCard/MyListItem/MyScrollViewer/MyMsg* 等 40+ 个
├─ PCLCS/                       C#：Java 探测、Minecraft 工具、启动包装器 jar（JavaWrapper/LwjglUnsafeAgent）
└─ MeloongCore/                 C# 子模块：跨项目基础设施（本克隆为空）
```

| 层 | 职责 | 关键实现 |
|---|---|---|
| `Modules/Base` | 与 MC 无关的运行时：路径、ini 配置、进程、HTTP、下载引擎、加载器、动画、日志 | `ModNet.vb` 1,933 行、`ModBase.vb` 1,216 行、`ModLoader.vb` 791 行 |
| `Modules/Minecraft` | 领域层：实例模型、版本 JSON、库/资源、登录、参数、启动、Java、整合包、崩溃 | `ModLaunch.vb` 2,221 行、`ModMinecraft.vb` 2,153 行、`ModDownload.vb` 1,389 行 |
| `Pages` | 视图 + 视图逻辑（MVP-ish，无 ViewModel） | 每个页面 `.xaml` + `.xaml.vb`，直接调 Module |
| `Controls` | 统一视觉/动画的可复用控件 | 自绘模板、`MyLoading`、`MyHint`、`MyMsgBox` |
| `PCLCS` | 需要 C# 生态或独立进程的部分 | `Java.cs`、`MCUtils.cs`、`JavaWrapper.jar` |

**评价**：这是典型 WinForms/WPF 时代的“Module 单例 + 页面直调”结构，没有依赖注入，也没有 domain/UI 边界；好处是调用链短、改起来快，坏处是 `ModLaunch` 里直接 `RunInUi(...)`、`FrmMain.PageStack`，UI 与业务强耦合。Rust 版应把“领域/计划/执行”做成纯库，UI 只订阅状态。

### A1.2 导航方式

- 顶级页是枚举 `PageType`：`Launch / Download / Link / Setup / Other`；次级页是 `InstanceSelect / TaskManager / InstanceSetup / ResourceDetail / HelpDetail`（`FormMain.xaml.vb:1183-1214`）。
- 页面用 `PageStackData { Page, Additional() }` 表示，`PageStack` 是**栈**：进入子页时 `Insert(0, PageCurrent)`，返回时 `RemoveAt(0)` 直到目标页（`FormMain.xaml.vb:1436-1458`）。标题栏的返回按钮调用 `PageBack()`。
- `PageChange(Stack, SubType)` 负责同步左侧栏选中态，`PageChangeActual` 负责真正换页；`PageChangeAnim` 做左右两栏的进出动画，并在换页时触发 `PageOnExit/PageOnEnter`（`FormMain.xaml.vb:1362-1545`）。
- 页面实例**懒加载**并缓存：`If FrmDownloadLeft Is Nothing Then FrmDownloadLeft = New PageDownloadLeft`；同一页面再次进入不重建，靠 `Reload()` 刷新。左侧栏 `MyPageLeft` / 右侧 `MyPageRight` 是固定容器，换页只替换 `ContentPresenter.Content`。
- `PageSubType` 的数值被硬编码为左侧 `StackPanel` 的下标（注释明确要求“数值必须与 StackPanel 中的下标一致”），属于典型的“枚举即布局”耦合。

**可借鉴**：单窗口 + 左右双栏 + 页面栈 + 懒加载页面实例 + 统一转场动画，这套对桌面启动器足够，且实现成本远低于路由框架。
**不要抄**：用枚举下标绑定布局；页面直接持有业务单例。

---

## A2. 启动流程

### A2.1 总览：一条带权重的加载器流水线

入口 `McLaunchStart`（`ModLaunch.vb:31`）做四件事：检查是否已在启动、必要时切换实例、检查实例锁、把 `InstanceSelect` 从页面栈里删掉（防止启动中切版本），然后启动 `McLaunchLoader`。真正的流水线在 `McLaunchStart(Loader)`（`ModLaunch.vb:106-208`）：

| # | 加载器 | 权重 | 阻塞 | 说明 |
|---|---|---|---|---|
| 1 | 获取 Java | 4 | 否 | `McLaunchJava`，选择/校验 Java |
| 2 | 登录 | 15 | 否 | `McLoginLoader`，与其它步骤并行 |
| 3 | 补全文件 | 15 | 否 | `DlClientFix`：库 + 资源索引 + 资源 |
| 3.5 | 内存优化 | 30 | 是（可选） | 仅在设置开启时插入，会 `EmptyWorkingSet` 类操作，显著拖慢启动 |
| 4 | 获取启动参数 | 2 | 是 | JVM + mainClass + game |
| 5 | 解压 Natives | 2 | 是 | 按大小跳过、清理多余 DLL |
| 6 | 预启动处理 | 1 | 是 | 显卡偏好、自定义命令等 |
| 7 | 执行自定义命令 | 1 | 是 | 启动前脚本 |
| 8 | 启动进程 | 2 | 是 | `ProcessStartInfo` |
| 9 | 等待游戏窗口 | 1 | 是 | `Watcher` 监视窗口与日志 |
| 10 | 结束处理 | 1 | 是 | 启动器隐藏/关闭、音乐、计数 |

关键设计：**`Block=False` 的加载器可以与后续步骤并行**（Java、登录、补全文件），`LoaderCombo` 用权重汇总进度；`McLaunchLoaderReal` 是实际组合加载器，UI 每 100 ms 刷新一次。这套“声明式任务树 + 权重”比 Prism 的 `Task` 串行链和 HMCL 的 `CompletableFuture` 链更容易做“总进度条不倒退”。

加载器体系的几个要点（`Modules/Base/ModLoader.vb`）：

- `LoaderBase`：`State`（Waiting/Loading/Finished/Failed/Canceled）、`Error`、`ProgressWeight`、`Block`、`Show`、`IsForceRestarting`、`CancellationTokenSource`；状态变化同时抛 UI 事件与工作线程事件，`MyLoading` 控件直接订阅。
- `LoaderTask(Of TIn, TOut)`：带输入/输出的任务；`ShouldStart` 做“同输入且已完成则复用结果”的缓存，`ReloadTimeout` 控制缓存有效期（下载源探测就靠它避免重复请求）。
- `LoaderCombo`：组合加载器；`Block=False` 的子加载器并行跑，`Block=True` 的按顺序跑；进度按 `ProgressWeight` 加权汇总；任一子加载器失败可决定是否中断整组。
- `LoaderDownload`：把 `NetFile` 列表交给 `NetManager`，把“文件级进度/速度/剩余时间”映射成加载器进度。
- `LoaderFolderRun`：对文件夹下每个子项跑同一个加载器（版本列表扫描、整合包逐版本检查）。

这套抽象让“启动”“下载”“扫描”“安装”共享同一套取消、重试、进度、错误 UI；Rust 版可以用 `trait Step { async fn run(&self, ctx) -> Result<Out>; fn weight(&self) -> f32; fn blocking(&self) -> bool; }` + `JoinSet` 复刻，但不要复刻它的全局单例与 UI 线程回调。


### A2.2 版本 JSON 解析

`McInstance`（`ModMinecraft.vb:218-630`）是惰性模型：

1. `GetJsonPath()`：优先 `<版本文件夹>/<名字>.json`；找不到就扫描同目录任意 `*.json`，要求含 `mainClass/type/id`，找到后警告并改用（兼容改名/残缺安装）。
2. `JsonText`：读文件后做“首 `{` 尾 `}`”快速检查；失败时**延时 2 s 重读一次**（杀软/下载器占用文件的常见问题），再失败才抛异常。
3. `JsonObject`（`ModMinecraft.vb:420-506`）：
   - 若含 `patches` 且不含 `time`，判定为 **HMCL 格式**，按 `priority` 排序后逐个 `Merge`，取 `id`，删 `inheritsFrom`。
   - 否则按 `inheritsFrom` 递归：子版本 `libraries` **在前**、父版本 `libraries` **在后**（注释解释：Java 8 以前 classpath 同名类取第一个），其余字段父版本先 `Merge`、子版本覆盖；检测自引用与嵌套依赖并报错。
4. `JsonVersion`：从版本 jar 内 `version.json` 读取（用于识别 jar 内声明的版本），失败则忽略。
5. 版本列表加载时**不解析每个 JSON**，而是读每个实例的 `PCL/Setup.ini` 缓存（`VersionVanillaName`、`VersionForge`、`ReleaseTime`、`Logo`、`State` 等），只有缓存缺失/状态为 Error 才 `Load()`（`ModMinecraft.vb:1030-1136`）。这是 PCL 冷启动快的关键之一。
6. `McVersion` 解析器把 `id`/`inheritsFrom`/jar 内 `version.json` 综合成 `Vanilla / Drop / ModLoaders / OptiFine / ReleaseTime / IsSnapshot / IsFool`，用于卡片分类与 Java 版本需求。

**规则引擎**（`ModMinecraft.vb:1554-1601`）非常简化：只处理 `os.name=windows`（`os.version` 用正则匹配当前系统版本），`os.arch` 只排除 x86/arm，`features` 只处理 `is_demo_user`（false）和 `quick_play*`（false）。`action=allow/disallow` 按顺序覆盖。**Rust 版必须实现完整语义**（os.name/version/arch、features 的 `has_custom_resolution`、`is_demo_user`、`quick_plays_support` 等）。

**库解析**（`ModMinecraft.vb:1653-1765`）：

- 支持 `downloads.artifact`、`downloads.classifiers[ natives.windows ]`、根级 `url` + Maven 坐标拼接三种来源。
- Natives 本地文件名把 `.jar` 替换为 `-<classifier>.jar`，`${arch}` 替换为 64/32。
- **按 `group:artifact` 去重，保留版本号更高者**（`McUtils.CompareVersionGE`）。这是对官方语义的偏离：Mojang 规范里同名不同版本可以共存且顺序有意义；PCL 用“去重降复杂度”，绝大多数情况下没事，但极端整合包可能踩坑。
- 大小/SHA1 从 JSON 读入 `McLibEntry`，供下载器校验。

**资源索引**（`ModMinecraft.vb:1930-2088`）：读 `assetIndex`（含 `id/url/sha1/size/totalSize`），按 hash 前两位分目录；支持 legacy `virtual/legacy` 映射；支持“后台每天更新一次索引”（`DlClientFix` 的 `DownloadAssetIndexInBackground`）。

### A2.3 补全与版本隔离

`DlClientFix`（`ModDownload.vb:54-156`）返回一个加载器列表：

1. **库**：`McLibNetFilesFromInstance` 生成 `NetFile` 列表（含 URL 多源、大小、SHA1），`LoaderDownload` 下载。
2. **资源索引**：可后台更新：先下到 `Cache\<name>_background`，下次启动时若比正式文件新则移动过去；每天只检查一次。
3. **资源**：`McAssetsFixList` 生成 `NetFile`，`CheckHash=true` 时把 SHA1 校验推迟到下载线程里做（利用多线程并行校验），否则先本地大小检查再决定是否下载。

**版本隔离**（`McInstance.PathIndie`，`ModMinecraft.vb:236-283`）是 PCL 很“务实”的一段：

- 若无显式设置：
  - 版本文件夹下存在 `mods/` 且有文件，或 `saves/` 且有子目录 → 自动隔离；
  - 否则按全局策略：0 关闭、1 仅隔离可装 Mod 的版本、2 仅隔离非正式版、3 非正式版或可装 Mod、4 全隔离。
- 结果写入 `PCL/Setup.ini` 的 `VersionArgumentIndieV2`，之后不再重算。
- 隔离时 `PathIndie = PathVersion`，否则 `PathIndie = McFolderSelected`。启动日志会打印“版本隔离：True/False”。

对比：Prism 是“实例即目录，天然隔离”；HMCL 是全局开关 + 每版本覆盖；PCL 用“看有没有 mods/saves”做启发式，减少新手误判，但会在“空 mods 文件夹”场景失效。

### A2.4 启动参数拼装

`McLaunchArgumentMain`（`ModLaunch.vb:1191-1218`）顺序：**JVM 参数 → mainClass → 游戏参数**，然后统一做占位符替换、含特殊字符加引号、拼成单个命令行字符串。

**JVM 参数**（`ModLaunch.vb:1221-1313`）：

- 新版：读 `arguments.jvm`，字符串或 `{rules, value}` 对象，沿 `inheritsFrom` 链逐层追加；对含空格且未加引号的参数补双引号（注释举了 1.17.1 的 `-Dos.name=Windows 10`）。
- 旧版：硬编码 `-XX:HeapDumpPath=...`、`-Djava.library.path=${natives_directory}`、`-cp ${classpath}`。
- 自定义 JVM 参数（全局 + 实例）先做变量替换，再与内置参数**一起重新分词、去重**。
- **JLW（JavaWrapper）**：非 GBK 编码时注入 `-javaagent:JavaWrapper.jar`，解决“中文路径/用户名导致 JVM 参数乱码”（JDK-8272352）；有自定义 `-javaagent` 时禁用并警告。
- **LUA（LwjglUnsafeAgent）**：检测到 `org.lwjgl:lwjgl:3.4.1` 时注入，规避 LWJGL 的 Unsafe 限制。
- **authlib-injector**：见 A4。
- **GC 策略**：按 Java 版本选 ZGC/G1GC（Java 21+ 分代 ZGC、15–20 非分代 ZGC、<15 G1GC；Win10 1809 以下强制 G1），先删掉已有的 GC 参数再追加，Java 24+ 加 `-XX:+UseCompactObjectHeaders`。
- **编码**：Java 18+ 用 `native.encoding`，17- 用 `file.encoding`；`GBK` 映射到 `GB18030`；设置 `stdout.encoding`/`stderr.encoding`（Java 19 前是 `sun.*`）；Java 18+ 加 `-Dfile.encoding=COMPAT` 兼容老 Mod。
- 追加 `-Dlog4j2.formatMsgNoLookups=true`。
- `-Xmx` 来自实例设置（GB → MB）。

**游戏参数**（`ModLaunch.vb:1404-1616`）：

- 旧版 `minecraftArguments` 直接拼，并**总是追加** `--height/--width`，靠后面的去重覆盖原值。
- 新版 `arguments.game` 同样支持 rules/字符串/数组，沿继承链追加。
- 自定义游戏参数（全局 + 实例）。
- **OptiFineForgeTweaker 修正**：Forge/LiteLoader + 非 mod 形式 OptiFine 时，把 `--tweakClass` 移到参数末尾；若发现错误的 `OptiFineTweaker`，还会**改写版本 JSON**（`FileUtils.Write`）修正为 `OptiFineForgeTweaker`。
- 全屏、自动进服：2023-05-04 之后的版本用 `--quickPlayMultiplayer <ip>`，老版本用 `--server/--port`；OptiFine 下提示可能不兼容。

**去重**（`ModLaunch.vb:1617-1658`）是 PCL 的“独门”：

- 区分“单参数”（`-XX:+UseG1GC`）与“键值对”（`--width 233`）。
- 游戏参数遇到重复键**用新值覆盖旧值**（注释：多个 `--width`/`--uuid` 会同时失效）；工具参数（`--tweakClass`）例外。
- JVM 参数重复键只保留第一个；键值完全相同则丢弃。
- 负数被特殊处理，避免 `-xPos 23 -xPos -50` 把 `-50` 当参数名。

**占位符替换**（`ModLaunch.vb:1506-1616`）：

- `${classpath_separator}` 固定 `;`；`${natives_directory}`、`${library_directory}`、`${game_directory}`、`${assets_root}`、`${game_assets}`（legacy）。
- `${auth_player_name}`、`${auth_uuid}`、`${auth_access_token}`、`${user_type}=msa`。
- `${launcher_name}=PCL`、`${launcher_version}=413`、`${version_name}`、`${version_type}`（空时删掉 `--versionType` 对，避免 MC 显示空类型）。
- `${resolution_width/height}`：默认 854×480；可跟随启动器窗口；对 1.12- + Java 8u200~321 做 DPI 修正。
- `${classpath}`：遍历 `McLibListGet`，跳过 natives，OptiFine 放在倒数第二位；每个路径做 **8.3 短路径**转换（`PathUtils.ToShortPath`）。
- `${primary_jar}`、`${assets_index_name}`。

**短路径**是 PCL 的重要兼容手段：`PathPure`（`ModBase.vb:791-802`）优先选“纯 ASCII 且可写”的目录（exe 目录 → AppData → Temp → `C:\ProgramData\PCL\`），用于放 `JavaWrapper.jar`、`authlib-injector.jar`、libwebp 等需要传给 JVM/系统 API 的路径；classpath 里每个库路径也尽量转 8.3，规避中文路径与命令行长度问题。

### A2.5 进程启动与日志

`McLaunchRun`（`ModLaunch.vb:2093-2141`）：

- 使用 **`java.exe` 而不是 `javaw.exe`**（注释 #6263：javaw 会丢失 stdout/stderr）。
- 环境变量：把 Java 的 `bin` 目录**前置**到 `PATH`；把 `appdata` 指向 `.minecraft` 根目录（老版本 MC 会把存档/资源写到 `%APPDATA%`，这样重定向可避免污染系统目录）。
- `WorkingDirectory = PathIndie`；`UseShellExecute=False`；`RedirectStandardOutput/Error=True`；`CreateNoWindow=True`；`StandardOutputEncoding/StandardErrorEncoding = 计算出的编码`。
- 启动后设置 `PriorityBoostEnabled`，按设置改 `AboveNormal/BelowNormal`；若此时用户取消，立即 `Kill()`。

`Watcher`（`ModWatcher.vb:66-330`）负责运行期：

- 异步读 stdout/stderr，逐行解析；维护 5 级启动里程碑（LWJGL 版本 → 用户设置 → OpenAL → 材质 atlas → 窗口出现），用于进度条。
- 崩溃识别靠日志关键字：`Crash report saved to`、`Could not save crash report to`、`Unable to launch`、`An exception was thrown...`；正常关闭识别 `Someone is closing me!` / `Restarting Minecraft with command`。
- 窗口识别靠类名 `GLFW30 / SDL_app / LWJGL / SunAwtFrame`，排除 `FML*`、`Quilt Loader`、隐藏的 SDL 窗口、`PopupMessageWindow`；找到后可按设置最大化、改标题（支持 `{date}/{time}` 占位符）。
- 退出码非 0 且处于 Running 状态（且版本年份 ≥2012）视为崩溃。
- 进程退出后触发 `McLaunchEnd`：按设置关闭/隐藏/最小化启动器、暂停/恢复音乐、启动计数。

**评价**：这套“日志里程碑 + 窗口类名 + 关键字”比读崩溃报告文件更实时，但非常 Windows 特化；Rust 版应保留“日志行 → 状态机”的思路，窗口探测用 `windows` crate 的 `EnumWindows/GetClassNameW`。

### A2.6 与 Prism / HMCL 不同的“务实”做法

| 主题 | PCL | Prism | HMCL | 评价 |
|---|---|---|---|---|
| 版本模型 | 官方 JSON + `inheritsFrom` 递归；兼容 HMCL `patches` | 私有 meta + 组件列表，官方 `arguments` 基本不解析 | 官方 JSON + 合并，支持多方格式 | PCL 最贴近官方，且兼容 HMCL |
| Rules | 只实现 Windows 子集，features 硬编码 | 组件级 rules，较完整 | 较完整 | PCL 不可移植；Rust 必须完整 |
| 库去重 | 按 `group:artifact` 保留高版本 | 按组件覆盖顺序 | 按坐标去重 | PCL 可能改变 classpath 语义 |
| Natives | 按大小跳过、删多余 DLL、8.3 短路径、失败重试 | 解压到实例 natives 目录 | 解压到 natives 目录 | PCL 对“杀软占用/中文路径”更鲁棒 |
| 启动并行 | Java/登录/补全并行，权重进度 | 任务串行为主 | Future 链，部分并行 | PCL 的 UI 进度体验最好 |
| 参数去重 | 自研分词 + 键值覆盖 + 负数处理 | Qt 参数列表，少量去重 | 参数列表 | PCL 处理第三方魔改参数更强 |
| 编码 | JLW `-javaagent` 包装 + `COMPAT` | 依赖 JVM 参数 | 设置 `file.encoding` | PCL 对中文环境最细 |
| 隔离 | 看 `mods/saves` 自动决定 | 实例天然隔离 | 全局开关 | PCL 对新手最友好，边界情况会误判 |
| 资源修复 | 跨 MC 文件夹按大小+SHA1 找同文件复制 | 仅当前实例校验 | 仅当前实例校验 | PCL 多文件夹用户受益明显 |
| 下载 | 单文件多线程分段 + 源隔离 + 自动回退 | 多文件并发，单文件单请求 | 多文件并发，单文件单请求 | PCL 在弱网/大文件上优势明显 |
| 元数据 | 直连 Mojang/BMCLAPI，无自建服务 | 强依赖自建 meta | 直连 + 镜像 | PCL 运维成本最低，但受官方抖动影响 |

---

## A3. 下载与镜像

### A3.1 下载源策略

三个独立设置（`Pages/PageSetup/Settings.vb:117-121`）：

- `ToolDownloadSource`（文件）：0 尽量镜像 / **1 优先官方，慢时换镜像（默认）** / 2 尽量官方。
- `ToolDownloadVersion`（版本列表）：同上，默认 1。
- `ToolDownloadMod`（社区资源）：0 尽量镜像 / 1 官方优先 / **2 尽量官方（默认）**，因为 Modrinth 的元数据镜像可能滞后。

URL 重写规则（`ModDownload.vb:1213-1316`）：

| 类别 | 官方 | 镜像 |
|---|---|---|
| assets | `resources.download.minecraft.net` | `bmclapi2.bangbang93.com/assets` |
| libraries | `libraries.minecraft.net` / `piston-*.mojang.com` | `bmclapi2.bangbang93.com/maven`（Forge/Fabric/NeoForge 优先）或 `/libraries` |
| meta/launcher | `piston-meta/launchermeta/launcher.mojang.com` | `bmclapi2.bangbang93.com` |
| Java runtime | `piston-data.mojang.com` | `bmclapi2.bangbang93.com` |
| OptiFine | `optifine.net` | `bmclapi2.bangbang93.com/optifine/...` |
| authlib-injector | `authlib-injector.yushi.moe` | `bmclapi2.bangbang93.com/mirrors/authlib-injector` |
| Modrinth | `api/cdn.modrinth.com` | `mod.mcimirror.top/modrinth`、`mod.mcimirror.top` |
| CurseForge | `api.curseforge.com`、`edge/forgecdn.net` | `mod.mcimirror.top/curseforge`、`mod.mcimirror.top` |
| 社区主页/配置 | GitHub raw | `gh-proxy.com/...`、`files.mcimirror.top` |

**Forge/Fabric/NeoForge 的 maven 不添加官方源**：因为这些产物的官方源在国内经常超时，直接只给镜像（`DlSourceLibraryGet` 的分支）。

### A3.2 多源失败转移（`DlSourceLoader`）

`ModDownload.vb:1317-1389` 实现了一个“按等待周期切换源”的状态机：

- 每个源有 `FailCount`，等待阈值 = `FailCount * 100 ms`（实际按 10 ms tick 计数）。
- 第一轮启动第一个源；若前序源全部失败，后续源立即启动；每个源失败/超时后启动下一个；全部失败则抛最后一个有意义的异常（若都没有则 `TimeoutException("下载源连接超时")`）。
- 成功后取消其余加载器。
- Mod 元数据请求更直接：`DlModRequest` 构造 `(url, timeout)` 列表，按设置排列，例如镜像优先时 `(镜像 10s, 镜像 10s, 官方 30s)`；逐个尝试，聚合异常（`ModDownload.vb:1176-1212`）。

### A3.3 下载引擎（`ModNet.vb`）

**并发模型**：全局 `NetTaskThreadCount`，上限 `NetTaskThreadLimit = ToolDownloadThread + 1`，默认 **64**。两个 `ThreadStarter` 线程每 20 ms 扫描队列：为等待文件开线程；若总速度低于 `256 KiB/s`，为进行中的文件**追加分段线程**。

**分段规则**：

- 最小片段 `FilePieceLimit = 256 KiB`（`ModNet.vb:772`）。
- 找“剩余未下载量最大”的线程，从其 `DownloadEnd - DownloadUndone * 0.4` 处切一刀，即新线程拿走剩余部分的 40%，旧线程保留 60%。这是无锁的“软工作窃取”，避免维护全局分片表。
- 首个线程从 0 开始，`Range: bytes=0-`；后续线程 `Range: bytes=start-`。
- **特定域名禁用多线程**：`pcl2-server`、`meloong.com`、`bmclapi`、`github.com`、`optifine.net`、`momot.rs`（`ModNet.vb:853-854`）。原因分别是签名 CDN、镜像限速、GitHub 不支持 Range/易 429、OptiFine 反盗链。
- BMCLAPI 请求后额外 `Sleep(100)` 降低频率（`ModNet.vb:1744,1764`）。

**超时与断链**：

- 连接/读取超时 = `min(max(平均连接时间, 15 s) * (1 + 源失败次数), 30 s)`（`ModNet.vb:909`）。
- 数据包间隔 > 5 s 且速度 < 1 KB/s → 主动抛 `TimeoutException` 断链。
- 首包前用 `CancellationTokenSource(Timeout)`，收到首包后重置为无超时，避免大文件被固定超时杀掉（#8488）。

**源级故障隔离**（`SourceFail`，`ModNet.vb:1070-1146`）：

- 每个源独立 `FailCount`；连续失败达到 `clamp(NetTaskThreadLimit,5,30)` 且几乎没下到数据 → 禁用该源。
- 404/502/416/“不支持 Range”/“空间不足” → 立即禁用。
- 403/429 **仅对非 BMCLAPI 源**禁用（BMCLAPI 高频请求会 403/429，但不应因此弃用）。
- 所有源都失败且未重试过 → 清空禁用状态，**全部按“不支持 Range”的单线程模式重试一次**；再失败才整体失败。
- 合并失败也会触发同样的重试。

**落盘与合并**：

- 每个线程写 `PathTemp\Download\<fileUuid>_<threadUuid>_<rand>.tmp`；完成后按偏移合并到目标文件；小文件/未知大小走 `MemoryStream` 单线程。
- 下载前清理 `PathTemp\Download`（每个进程只清一次）。
- 首线程拿到 `ContentLength` 后：校验 `MinSize/ActualSize`；>50 MiB 时检查目标盘/临时盘剩余空间（临时盘按 1.1 倍估算）。

**限速**：`NetTaskSpeedLimitHigh` 由设置映射（0.1–20 MB/s 或无限），每 100 ms 给 `NetTaskSpeedLimitLeft` 加 `high/10`，线程发送前自旋等待配额；粒度 16 KiB。

**校验**：

- 首线程用 `ContentLength` 对 `FileChecker.ActualSize` 做“必须相等”校验；`MinSize` 做下限校验。
- 资源/库的 SHA1 在下载完成后由 `FileChecker.Check` 校验（大小 + SHA1）；校验失败视为该源失败并重试。
- 没有全局“下载后统一 repar 扫描”，修复发生在“启动前补全”阶段。

**跨实例去重**（`CheckExistingFiles`，`ModNet.vb:1474-1565`）：

- 在**所有已配置的 MC 文件夹**中找同名/同大小/同 SHA1 的文件，找到就直接 `FileUtils.Copy`，标记 `IsCopy` 完成，不下载。
- 分类查找：assets/libraries 按相对路径；versions 按各版本目录；mods/resourcepacks 等按“大小 + SHA1”在同类目录里找。
- 每 40 个文件一个线程，最多 8 线程。
- 这是 PCL 相对 Prism/HMCL 的明显优势：多文件夹/多整合包用户能省大量重复下载。代价是复制而非硬链接，占额外磁盘。

### A3.4 对国内网络的适配总结

1. **默认官方优先、慢时换镜像，但不盲目**：文件/版本列表默认“优先官方，加载缓慢时换镜像”，社区资源默认“尽量官方”（镜像元数据可能滞后）。
2. **按类别选镜像路径**：maven / libraries / assets / meta 分别映射，而不是简单替换域名。
3. **多源顺序回退 + 每源独立超时**：避免一个慢源拖死整个下载（不是并发竞速，是带超时的接力）。
4. **对镜像限速友好**：BMCLAPI 单线程 + 100 ms 间隔；GitHub/optifine 单线程。
5. **对反盗链友好**：自定义 UA `PCL2/<ver>`、Referer、CurseForge `x-api-key`；百度网盘 UA 特判。
6. **对弱网友好**：低速断链重连、源级隔离、无 Range 回退、断点临时文件、磁盘空间预检。
7. **对证书/代理的处理**：`ServicePointManager` 全局连接数 10000、关 Nagle、开 DNS round-robin、ReusePort；另提供“登录时关闭 SSL 校验”开关（`ShouldValidateSslCertificateOnLogin`）——这是 MITM 风险，不建议照抄。
8. **对时区/语言友好**：`Accept-Language: zh-CN`（部分登录接口 `en-US`）。

---

## A4. 账户与登录

`McLoginData` 有三个子类：`McLoginMs`（微软）、`McLoginLegacy`（离线）、`McLoginServer`（authlib-injector / Nide / 统一通行证）。`McLoginStart` 是统一入口，`McLoginLoader` 是加载器；登录结果 `McLoginResult { Name, Uuid, AccessToken, ClientToken, Type, ProfileJson }`。

### A4.1 微软登录：标准 device code 六步

`MsLoginStep1New` ~ `MsLoginStep6`（`ModLaunch.vb:882-1100`）：

| 步骤 | 请求 | 关键点 |
|---|---|---|
| 1a | `POST https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode`，body `client_id=...&tenant=/consumers&scope=XboxLive.signin offline_access` | 返回 `user_code/device_code/verification_uri/interval/expires_in`；弹窗显示 code、自动打开网页、复制 code；按 `interval` 轮询 token 端点；`authorization_pending` 继续；未知错误最多重试 3 次；`expired_token`/`authorization_declined` 失败。**PCL 未显式处理 `slow_down`**（规范要求增大轮询间隔），这是个小缺口，Rust 版应补上 |
| 1b | `POST https://login.live.com/oauth20_token.srf`，`grant_type=refresh_token` | 刷新；错误含 `must sign in again` / `password expired` / `refresh_token is not valid` → 返回 `Relogin` 让用户重走 device code；`Account security interrupt`、`service abuse` 单独提示 |
| 2 | `POST https://user.auth.xboxlive.com/user/authenticate`，`RpsTicket = "d=" + access_token`（已有 `d=` 则不重复加），`RelyingParty=http://auth.xboxlive.com`，`TokenType=JWT` | 拿 `Token`（XBL） |
| 3 | `POST https://xsts.auth.xboxlive.com/xsts/authorize`，`SandboxId=RETAIL`，`UserTokens=[XBL]`，`RelyingParty=rp://api.minecraftservices.com/` | 拿 `Token`（XSTS）与 `DisplayClaims.xui[0].uhs`；错误码 2148916227 封禁、2148916233 无 Xbox 账户、2148916235 地区限制、2148916238 未成年 |
| 4 | `POST https://api.minecraftservices.com/authentication/login_with_xbox`，`identityToken = "XBL3.0 x=<uhs>;<xsts>"` | 拿 MC `access_token` + `expires_in`；429 频繁、403 IP 异常、503 Mojang 故障、`ACCOUNT_SUSPENDED` 封禁 |
| 5 | `GET https://api.minecraftservices.com/entitlements/mcstore`，`Authorization: Bearer <mc token>` | `items` 为空 → 未购买/Game Pass 到期；**这一步也会刷新 XGP 权益** |
| 6 | `GET https://api.minecraftservices.com/minecraft/profile` | 拿 `id`（无连字符 UUID）与 `name`；404 → 未创建玩家档案 |

> 注：PCL 的刷新走 `login.live.com/oauth20_token.srf`（历史端点），device code 走 `login.microsoftonline.com/consumers/...`。Rust 新实现建议统一用 `/consumers/oauth2/v2.0/token`，减少端点和错误格式差异；若沿用 `login.live.com`，要按它的错误字符串做映射。

**缓存**：`CacheMsV2Access/Uuid/Name/ProfileJson/Expires`（过期时间 = `expires_in + now - 3600`，提前 1 小时视为过期）；`LoginMsJson` 以用户名为 key 存 refresh token。启动时若 access token 未过期且用户名匹配，直接复用，不请求网络。

**Client ID**：开源版 `ModSecret.vb:10` 从环境变量 `PCL_MS_CLIENT_ID` 读取，默认空。这说明 PCL 不把 client id 写进开源仓库。Rust 版同样应通过编译期 `env!`/配置注入，不能硬编码到仓库。

### A4.2 离线登录

`McLoginLegacyStart`（`ModLaunch.vb:685-705`）：

- `Name` 用户输入；`Uuid = McLoginLegacyUuidWithCustomSkin(Name, SkinType, SkinName)`。
- 默认 UUID 是 Java 的 `UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF-8))`（MD5 版本 3）；若用户选了自定义皮肤，则用皮肤名参与计算，保证不同皮肤得到不同 UUID。
- `AccessToken = ClientToken = Uuid`（离线模式占位）。
- 保存最近用户名列表 `LoginLegacyName`，用 `¨` 分隔。

Rust 版注意：MD5 在 `md-5` crate；UUID v3 需要手工设置 version/variant 位，或直接用 `uuid::Uuid::new_v3(&Uuid::NAMESPACE_OID, ...)`（但 Java 的算法是裸 MD5，不是标准 v3 命名空间，需按字节复刻并设置版本位）。

### A4.3 第三方认证（authlib-injector）

- 页面 `PageLoginAuth` 收集用户名/密码，服务器地址来自实例设置 `VersionServerAuthServer` 或全局 `CacheAuthServerServer`，API base 为 `<server>/authserver`。
- 登录走标准 Yggdrasil：`/authenticate`、`/refresh`、`/validate`、`/invalidate`；失败会 invalidate 后重试登录（`McLoginRequestValidate/Refresh/Login`，`ModLaunch.vb:706-880`）。
- 启动时（`McLaunchArgumentsJVM`）：
  1. 若 Java ≥ 6，加 `-Djavax.net.ssl.trustStoreType=WINDOWS-ROOT`（信任系统根证书，解决自签/企业代理，注释 #5252）。
  2. `NetRequestByClientRetry(server)` **预取** Yggdrasil 元数据（`/` 根返回的 JSON）。
  3. 加 `-javaagent:${pure_directory}\authlib-injector.jar=<server>`、`-Dauthlibinjector.side=client`、`-Dauthlibinjector.yggdrasil.prefetched=<base64(元数据)>`。
- authlib-injector.jar 的获取：`McLibNetFilesFromInstance` 里请求 `https://authlib-injector.yushi.moe/artifact/latest.json`（备份 BMCLAPI 镜像），下载到 `PathPure\authlib-injector.jar`（`ModMinecraft.vb:1805-1815`）。
- 统一通行证（Nide）类似：`-javaagent:${pure_directory}\nide8auth.jar=<server>`。

### A4.4 安全评估

| 项 | PCL 现状 | 风险 | Rust 版建议 |
|---|---|---|---|
| MS refresh token | 明文 ini（`LoginMsJson`） | 本机任意进程可读；备份/同步泄露 | `keyring`（Windows Credential Manager / macOS Keychain / Secret Service），失败再退回加密文件 |
| MC access token | 明文 ini + 启动日志 | 日志脱敏只对 `McLaunchLog` 做 `FilterAccessToken`；崩溃报告可能带出 | 永不落日志；`SecretString` + `zeroize` |
| 密码（authlib/Nide） | 明文 ini（`LoginAuthPass`） | 同上 | 只存 refresh/access，不存密码；必须记住时用 keyring |
| 日志脱敏 | `FilterUserName`/`FilterAccessToken` 替换为 `*` | 正则覆盖有限 | 统一在 tracing layer 做 redaction，默认不打印 token |
| SSL | 可关闭校验（设置项） | MITM | 默认开启；企业代理用系统根证书而不是关闭校验 |

---

## A5. “轻量、单文件、免安装”的具体手段

### A5.1 事实

- **不是 .NET 单文件发布**：`vbproj` 目标 `v4.8`，`App.config` 的 `supportedRuntime` 是 `.NETFramework,Version=v4.6.2`；依赖程序集（Newtonsoft.Json、NAudio、Ookii.Dialogs、Imazen.WebP、CacheCow、PCLCS、MeloongCore 等）以 `EmbeddedResource` 嵌入，运行时通过 `AppDomain.AssemblyResolve` 从 `My.Resources` 加载（`Application.xaml.vb:217-233`）。
- **exe 5.70 MiB**（发布物），zip 2.54 MiB；对比我们实测的 egui hello 5.15 MiB / gzip 2.11 MiB，量级相同。PCL 用 .NET Framework 的共享运行时换来了小体积，代价是**必须装 .NET Framework 4.8**（Win10 1903+ / Win11 自带；Win7/8 需另装，PCL 会检测 `Release >= 528040` 并提示）。
- **为什么不用 .NET 的 `PublishSingleFile`**：.NET Framework 4.8 不支持；若迁到 .NET 8，framework-dependent 单文件约 10–20 MB（仍要装 .NET 8 Desktop Runtime），self-contained 单文件通常 **>100 MB**（含 WPF 原生库）。PCL 选择“老框架 + 自研资源嵌入”，是在“体积/兼容性/开发成本”之间的务实取舍，而不是技术先进。
- **内存**：PCL 不主打低内存；WPF + .NET Framework 的基线通常在 **100 MB 量级**（未在 Windows 实测，不作精确断言）。它的“轻”主要体现在**体积、免安装、冷启动**，不是常驻内存。
- **免安装**：所有配置在 exe 同级的 `PCL\` 目录（`PCL\Setup.ini`、`PCL\PCL.ini`、`PCL\Pictures`、`PCL\Musics`），不写注册表（仅 `HKCU\Software\PCLDebug` 存自定义事件变量）；缓存默认 `%TEMP%\PCL\`，可改到任意目录；删除文件夹即卸载。
- **启动速度**：两阶段初始化 + `SplashScreen`；日志打印“第一阶段加载用时”；页面懒加载；版本列表读 `Setup.ini` 缓存而不是解析 JSON；`MyVirtualizingElement` 虚拟化长列表。
- **内存**：WPF + .NET Framework 的基线不低（无法在 Linux 实测，本文不给未经验证的数字）。PCL 的“省内存”主要来自：不常驻 JVM、图片按需解码、下载缓存清理、页面实例复用；但它没有做激进的对象池/纹理压缩。

### A5.2 值得 Rust 版学习

1. **单文件 + 便携目录**：配置与数据放在 exe 同级 `data/`，不写注册表；缓存放在系统临时目录并可配置。Rust 天然静态链接，单文件比 PCL 更彻底（无需运行时）。
2. **启动分阶段 + 首帧优先**：先画壳（标题栏/背景/骨架），再异步加载实例列表、Java、账户；PCL 的 `SplashScreen + 两阶段日志` 可直接对应到 Rust 的“先 `eframe::run_native` 出窗，再 `tokio::spawn` 初始化”。
3. **列表缓存**：为每个实例缓存一份轻量元数据（版本号、loader、release time、logo、错误状态），列表页不解析完整 JSON。Rust 可用 `redb`/`rusqlite` 或每实例 `instance.toml` + 内存索引。
4. **页面懒加载与复用**：不要一次性构造所有页面；用 `Option<Box<dyn Page>>` + 首次进入时初始化。
5. **虚拟化列表**：实例/Mod 列表只渲染可视行；egui 用 `ScrollArea::show_rows`，iced 用 `Lazy`/`responsive`。
6. **资源内嵌**：图标、字体子集、默认皮肤用 `include_bytes!`/`rust-embed`，避免安装包散文件。
7. **短路径/ASCII 兜底**：给 JVM 的 `-javaagent`、natives 路径准备 UTF-8 安全目录（Windows 上用 `GetShortPathNameW`，或把运行期文件放到 `%LOCALAPPDATA%` 的 ASCII 目录）。

### A5.3 不要照抄

- **AssemblyResolve 动态加载**：调试困难、启动反射开销、单文件校验复杂；Rust 静态链接没有这个问题。
- **全局可变单例 + UI 线程直达**：Rust 借用检查会逼你设计消息通道，反而更好。
- **用 ini 存 token**：见 A4.4。
- **简化 rules / 库去重**：Windows-only 的捷径不能带进跨平台 Rust 版。

---

# B. Rust 生态与竞品

## B1. 现有 Rust Minecraft 启动器/相关项目

数据抓取时间 2026-10-20（GitHub API / Releases API）。星数为当时值；“Windows 体积”取最新 release 中 Windows 资产，未注明则无对应资产。

| 项目 | 星 | GUI / 形态 | 许可 | 维护 | Windows 发布物体积 | 关键结论 |
|---|---:|---|---|---|---:|---|
| **Modrinth App（Theseus）** `modrinth/code` | 2,455 | **Tauri 2 + Vue 3/Vite**，Rust 后端 `theseus` crate | GPL-3.0 | 活跃（2026-10-20） | 安装包 **25.1 MiB** | 最成熟的 Rust 启动器；Tauri 换来 UI 效率，但 WebView2 依赖 + 内存/冷启动代价；core 与 GUI 分离（`theseus` 库 + `theseus_gui`）值得学 |
| **PandoraLauncher** `Moulberry/PandoraLauncher` | 933 | **gpui**（Zed 的 GPU 框架）+ gpui-component | MIT | 活跃 | 便携 exe **53.4 MiB**，安装器 13.6 MiB | 证明 gpui 能做出完整启动器；原生、性能导向；但 gpui 无稳定版、git 依赖、构建重、Linux 支持弱；53 MB 便携包说明“资源+gpui”不轻 |
| **QuantumLauncher** `Mrmayman/quantumlauncher` | 196 | **iced 0.13**（wgpu/tiny-skia 可选） | GPL-3.0 | 活跃 | zip **8.1 MiB** | 与 nailong 最接近的形态：iced + 模块化 core（ql_core/ql_instances/ql_mod_manager/ql_java_handler）；8 MB 含资源，说明 iced 能达到目标 |
| **OneLauncher** `Polyfrost/OneLauncher` | 156 | **Freya**（Skia）+ winit | GPL-3.0 | 活跃 | 便携 exe **19.7 MiB** | Skia 渲染好看但体积大；Freya 生态小、API 不稳；适合参考 UI，不适合作为轻量基线 |
| **LiquidLauncher** `CCBlueX/LiquidLauncher` | 288 | **Tauri + Svelte 4** | GPL-3.0 | 活跃 | NSIS **8.0 MiB**，MSI 10.3 MiB | Tauri 也能做到 8 MB 安装包；依赖 WebView2；认证用自家 `minecraft-auth-rs` |
| **portablemc** `theorzr/portablemc` | 553 | CLI（Rust 重写） | Apache-2.0 | 活跃 | 无 GUI | 启动逻辑/版本解析的干净参考；无 UI 包袱 |
| **Nitrolaunch** `Nitrolaunch/nitrolaunch` | 171 | CLI + 服务端管理 | GPL-3.0 | 活跃 | — | “启动器 + 服务器管理器”一体化；core 设计可读 |
| **conic launcher** `conic-apps/launcher` | 17 | **Slint 1.18** | GPL-3.0 | 活跃 | 无 release | 小而新的 Slint 实践；Slint 的声明式 UI + 原生渲染，适合设计驱动团队 |
| **HelixLauncher** | 68 | Rust（自绘/iced 系） | MPL-2.0 | 活跃 | 无 release | 早期项目，参考价值有限 |
| **SwiftLauncher** | 26 | **iced 0.13** | 无 | 2026-07 后停更 | exe **67.7 MiB**（异常大，疑似未 strip/带资源） | 反例：iced 不优化也能 60 MB+；体积取决于构建配置与资源 |
| **basalt-launcher** | 100 | Rust（Tauri？） | GPL-3.0 | 活跃 | 无 release | 功能全，尚早期 |
| **shard** | 79 | Rust + 去重存储 | MIT | 活跃 | — | 内容寻址的 mod 存储，解决多实例重复；可借鉴 Pandora 的 reflink/hardlink 思路 |
| **SharpCraftLauncher (scl)** | 192 | Rust + 自绘 | LGPL-3.0 | **已归档（2024-04）** | — | 历史项目，说明“自绘 UI + 启动器”维护成本高 |

**关键结论**

1. **生产级项目都自研 core**：Theseus、Pandora、Quantum、Liquid 都有独立 core crate；没有项目把启动流程完全交给现成库。`lighty-launcher`/`mc-launcher-core` 仍属早期。
2. **GUI 分布高度分散**：Tauri（Modrinth、Liquid）、gpui（Pandora）、iced（Quantum、Swift）、Freya（OneLauncher）、Slint（conic）各有生产案例——说明没有“唯一正确答案”，要按团队与指标选。
3. **体积差异主要来自 GUI 栈与资源，不是 Rust 本身**：8 MB（iced/Tauri）到 53 MB（gpui）都有；iced+资源可做到 8 MB，egui hello 5 MB，说明目标 15 MB 合理。
4. **Tauri 的安装包不大，但运行时依赖 WebView2**；对“下载一个 exe 双击即用、无需任何运行时”的定位是硬伤。
5. **gpui 不适合小团队**：无 crates.io 稳定版、API 频繁 breaking、构建依赖 Zed 巨型 workspace；Pandora 是“有能力的团队 + 长期投入”的产物。

**B1.1 架构观察（值得抄/不值得抄）**

- **Modrinth App**：`apps/app` 是 `theseus_gui`（Tauri 命令层）+ `theseus` 核心库；`daedalus` 封装 Modrinth API；用 `async_zip`、`sha2`、`tokio`、`tracing`、`tauri-plugin-single-instance/updater`。**core/GUI 分离**是最值得抄的；不值得抄的是“所有 UI 都走 IPC + WebView”，启动器不需要这层复杂度。
- **Pandora**：workspace 分 `backend / bridge / frontend / pandora_launcher / auth / nbt / schema`；`gpui` 做 UI，`keyring` 存凭据，reflink/hardlink 做 mod 去重，sandbox 隔离。**“凭据进 keyring + 文件级去重”** 两点直接可用；gpui 的技术风险不要抄。
- **QuantumLauncher**：`ql_core / ql_instances / ql_mod_manager / ql_servers / ql_packager / ql_java_handler` + `quantum_launcher`（iced）。**按领域拆 crate、GUI 只做编排**，与 nailong 的目标结构最接近。
- **LiquidLauncher**：Tauri + 自家 `minecraft-auth-rs`；证明“认证独立成 crate”可行，但 WebView 依赖不适合轻量目标。
- **portablemc / Nitrolaunch**：CLI 优先，core 无 UI 依赖；适合参考“启动计划/安装计划”的纯函数设计。


---

## B2. GUI 选型对比

### B2.1 实测方法与结果

同一台机器、同一 Rust 版本、同一 release profile（`opt-level="z"`, `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`），每个框架一个“显示一行中英文文本”的窗口；Xvfb 1280×800 + llvmpipe。启动时间 = `spawn` 到 `xdotool search --name hello` 成功；RSS 分别取“窗口刚出现”和“静置 4–5 s”。

| 框架/配置 | 二进制 | gzip -9 | 启动到窗口 | RSS（窗口出现） | RSS（静置后） | 线程 |
|---|---:|---:|---:|---:|---:|---:|
| **egui 0.37 + eframe(glow)** | **5.15 MiB** | 2.11 MiB | **36–40 ms** | 71–85 MiB | 107 MiB | 34 |
| egui 0.37 默认 release（未 strip/LTO） | 8.67 MiB | — | — | — | — | — |
| **iced 0.13 + tiny-skia** | **3.85 MiB** | 1.65 MiB | **21–22 ms** | **30.2–30.7 MiB** | 30.7 MiB | 34 |
| iced 0.13 + wgpu + tiny-skia | 6.02 MiB | — | 98–115 ms | 155 MiB | 217 MiB | 71 |
| **slint 1.18 + femtovg** | **9.72 MiB** | 4.94 MiB | 88–101 ms | 85–96 MiB | 129 MiB | 38 |

> 说明：iced 实测为 0.13（0.14 的 API/依赖略有差异，量级预计相同）；llvmpipe 会为 OpenGL/wgpu 分配大量缓冲，iced+wgpu 的 155–217 MiB 明显被软件 Vulkan/GL 放大；egui 的 107 MiB 同理。**在 Windows 有 GPU 的机器上，egui 空闲通常 40–80 MiB、iced+wgpu 60–120 MiB、slint 50–90 MiB**（经验区间，非本文实测）。tiny-skia 是 CPU 渲染，不受 GPU 驱动影响，所以 30 MiB 很可信；代价是动画/滚动时 CPU 占用更高。

**Windows 真实发布物对照**（比 hello 更有说服力）：

| 应用 | GUI | Windows 资产 |
|---|---|---:|
| PCL 2.99 | WPF/.NET Framework | **5.70 MiB 单 exe** |
| LiquidLauncher | Tauri | 8.0 MiB setup / 10.3 MiB MSI |
| QuantumLauncher | iced | 8.1 MiB zip |
| OneLauncher | Freya/Skia | 19.7 MiB exe |
| Modrinth App | Tauri | 25.1 MiB setup |
| Pandora | gpui | 53.4 MiB 便携 exe |

### B2.2 逐项对比

| 维度 | egui | iced | slint | Tauri (wry) | gpui | Dioxus |
|---|---|---|---|---|---|---|
| 渲染 | 即时模式，glow/wgpu | 保留模式，wgpu/tiny-skia | 声明式 DSL，femtovg/skia/software | 系统 WebView | GPU（Metal/DX/Vulkan） | 默认 WebView；Blitz(WGPU) 实验 |
| 二进制（hello，Win/Linux 量级） | **5–8 MB** | **4–8 MB** | **10–15 MB** | 2.5–8 MB（不含 WebView2） | 20–50 MB+ | 5–15 MB（WebView 版） |
| 空闲内存 | 40–90 MB | 30–80 MB（tiny-skia 最低） | 50–100 MB | 100–300 MB+（多进程） | 80–200 MB | 100–300 MB |
| 冷启动 | **<100 ms** | **<100 ms** | 100–200 ms | 300 ms–2 s+（WebView2 初始化） | 100–300 ms | 300 ms–2 s |
| 中文输入（IME） | 0.37 已支持 composition/candidate 定位；Windows 有 winit 光标 bug，提供开关 | 0.14 加入 IME 支持（#2777） | 支持，但 CJK 字体需显式指定/打包 | WebView 原生，最稳 | 依赖平台实现，Windows 较新 | WebView 稳；Blitz 弱 |
| 中文字体 | 0.37 起可用系统字体 provider；之前必须自带 | cosmic-text 系统字体回退，默认可用 | 需指定 font-family，Linux 常缺中文字体 | 系统字体，稳 | 需自带/系统 | WebView 稳 |
| 主题/自定义绘制 | **极强**（Painter 直接画） | 强（canvas、自定义 widget） | 强（DSL + 自定义组件） | **最强**（CSS） | 强（但 API 不稳） | 强（CSS） |
| 开发效率 | 高（即时模式，改完即见） | 中（Elm 架构，状态/消息样板多） | 中（DSL + 代码生成，设计协作好） | **最高**（Web 技术栈） | 低（文档少、breaking） | 高（React 风格） |
| 打包复杂度 | 低（单 exe） | 低 | 低（build.rs 编译 DSL） | 中（WebView2、前端构建、签名） | 高（git 依赖、平台后端） | 中（CLI + WebView） |
| 生态/维护 | 极活跃（0.37，2026-10） | 活跃（0.14，2025-12；0.15-dev） | 活跃（1.18.1，2026-09） | 极活跃（2.11 稳定，3.0 alpha） | 活跃但无稳定发布（0.2.2，2025-10） | 活跃（0.8 alpha） |
| 适合 nailong | **主选** | **备选（最强轻量数据）** | 设计驱动备选 | 不推荐（WebView2） | 不推荐 | 不推荐 |

### B2.3 关键细节

**egui（推荐主选）**

- 优点：纯 Rust、单二进制、即时模式开发极快、`Painter` 可画任意形状/渐变/自定义控件；0.37 新增系统字体 provider（`egui_system_font_provider`，基于 fontique），**终于能在 Windows/macOS/Linux 直接用系统中文字体**，不必打包 10 MB 字体；0.37 还改进了 IME composition 与候选窗定位。
- 缺点：即时模式每帧重算布局，复杂页面（长列表、富文本）需要手动虚拟化/缓存；默认每帧重绘会耗电，需 `request_repaint_after` 降频；无原生无障碍（AccessKit 有，但不如保留模式完整）；默认字体不含 CJK，Linux 无中文字体时仍需兜底。
- 体积：hello 5.15 MiB；加图标、字体子集、业务代码预计 7–10 MB，满足 ≤15 MB。
- 适合：启动器这种“表单 + 列表 + 少量动画”的 UI；团队需要快速迭代。

**iced（推荐备选，若内存/体积优先级最高）**

- 优点：Elm 架构状态清晰，保留模式只重绘脏区；`tiny-skia` 纯 CPU 渲染，**无 GPU 驱动依赖**（老机器/虚拟机友好），实测 3.85 MiB / 21 ms / 30 MB，是全部选项里最轻的；cosmic-text 自带系统字体回退，中文开箱可用；0.14 加入 IME、响应式渲染、热重载、测试工具。
- 缺点：消息/状态样板多，复杂自定义控件不如 egui 直接；wgpu 后端在软件渲染下很重（217 MB），必须显式选 tiny-skia；生态比 egui 小。
- 适合：把“低配机器 + 内存预算”放在第一位，且团队能接受 Elm 架构。

**Slint**

- 优点：声明式 DSL，设计师可读；编译期生成代码，运行时小（MCU 级 300 KiB 说法是嵌入式，不含桌面渲染栈）；主题/动画/布局成熟；1.19（未发布）优化了启动与 CJK 字体回退。
- 缺点：桌面 hello 9.72 MiB（femtovg），比 egui/iced 大；CJK 需要显式 `font-family` 与字体打包（Linux 常缺）；自定义绘制要用 `Path`/`Image`，不如 egui Painter 灵活；Rust 侧调试 DSL 绑定较绕。
- 适合：UI 设计复杂、需要设计/开发协作，且能接受 10–15 MB。

**Tauri**

- 优点：Web 前端开发效率最高，CSS 主题/动画无上限；安装包 8–25 MB；生态成熟（单实例、更新器、托盘、深链插件齐全）。
- 缺点：**WebView2 运行时依赖**（Win11 自带；Win10 1803+ 可能没有；Win7 不支持）；固定版本运行时 180 MB、离线安装器 127 MB、引导器 1.8 MB；WebView 多进程内存 100–300 MB；冷启动 300 ms–2 s，且有“以管理员运行慢 20 s”的已知 bug；无法做到真正“单 exe 便携”（WebView2 不能塞进 exe）。
- 结论：与 nailong 的“单文件、无运行时依赖、低内存”冲突，不推荐。

**gpui**

- 优点：Zed 同款 GPU 渲染，性能好；Pandora 证明可做完整启动器；gpui-component 提供组件。
- 缺点：无 crates.io 稳定版（0.2.2，2025-10），项目普遍 git pin；API 频繁 breaking；构建依赖 Zed 大仓库；Windows 后端较新、Linux 支持弱；文档少；Pandora 便携包 53 MB。不适合小团队/轻量目标。

**Dioxus**

- 优点：React 风格、全栈、热重载；Web 版极强。
- 缺点：桌面默认仍是系统 WebView（同 Tauri 的依赖问题）；Blitz 原生渲染器实验性；体积/内存不占优。不推荐作为“轻量原生”主选。

### B2.4 明确推荐

- **主选：egui/eframe 0.37 + glow**（若某些老 GPU 的 OpenGL 3.3 不可用，提供 wgpu/DX12 回退）。理由：单文件、5 MB 量级、40 ms 级启动、开发效率最高、Painter 能满足 PCL 级别的自绘 UI；系统字体 provider 解决了 CJK 的最大短板。它与现有设计文档的 D1 决策一致。
- **备选（且是“硬指标兜底”）：iced 0.14 + tiny-skia**。实测最轻（3.85 MB / 21 ms / 30 MB）、无 GPU 依赖、系统字体回退；保留模式天然适合“空闲 <1% CPU”。如果 egui 在 Windows 实机的 20 实例场景超过内存/CPU 预算，切 iced 的代价主要在设计系统重写，业务层（`LaunchPlan`/状态机/下载器）不受影响。
- **建议的决策方式**：不要用本文的 Linux/llvmpipe 数字直接定生死；在 Windows 10/11 实机各跑一个 spike（同样的页面骨架：20 个实例、图标、滚动、下载进度），记录 `Working Set`、`Private Bytes`、首帧时间、空闲 CPU 10 分钟均值，再签字。**在 spike 之前，代码结构必须保证 GUI 可替换**（领域层不依赖 egui 类型）。
- **不选：Tauri、gpui、Dioxus**；**Slint 作为设计驱动备选**，但需接受 10 MB 与字体打包。

---

## B3. 关键 crate 调研

版本/下载量为 2026-10-20 的 crates.io 数据；“成熟度”按维护频率、下载量、API 稳定性综合。

### B3.1 运行时与网络

| crate | 用途 | 版本 | 成熟度/备注 |
|---|---|---|---|
| `tokio` | 异步运行时 | 1.54.0 | 事实标准；启用 `rt-multi-thread, fs, process, sync, time, io-util` |
| `reqwest` | HTTP 客户端 | 0.13.5 | 成熟；默认 `rustls-tls` 可避免 OpenSSL；需要 `stream`、`json`、`gzip`、`brotli` |
| `rustls` | TLS | 0.23.x 稳定（0.24 仍 dev） | 默认 `aws-lc-rs` 需要 cmake/NASM；Windows 构建可换 `ring` 减少工具链依赖 |
| `rustls-native-certs` | 系统根证书 | 0.8.5 | **国内/企业代理必加**；否则 rustls 不信任系统安装的根证书，登录/下载会失败 |
| `webpki-roots` | 内置 Mozilla 根 | 1.0.10 | 与 native-certs 二选一或叠加 |
| `reqwest-middleware` + `reqwest-retry` | 重试/中间件 | 0.5.2 / 0.9.1 | 可选；PCL 式“按源重试”更适合自研，不一定要 |
| `governor` | 限速（GCRA） | 0.10.4 | 成熟；可做全局令牌桶，替代 PCL 的手写限速 |
| `tokio-util` | `CancellationToken`、`codec` | 0.7.20 | 取消传播必备 |
| `futures-util` | `StreamExt`、`join_all` | 0.3.34 | 下载并发编排 |
| `bytes` | 零拷贝 buffer | 1.12.1 | 下载/解压 |
| `http` | 类型 | 1.x | 与 reqwest 配套 |

### B3.2 数据与文件

| crate | 用途 | 版本 | 成熟度/备注 |
|---|---|---|---|
| `serde` + `serde_json` | JSON | 1.0 / 1.0.154 | 版本 JSON 足够；`preserve_order` 可选 |
| `simd-json` | SIMD JSON | 0.18.1 | 快 2–3 倍，但要求 mutable input、对小型 version.json 收益有限；**不必用** |
| `zip` | ZIP 读写 | 9.0.5 | 成熟；0.6→9 API 变化大，注意 `ZipArchive::by_index` 返回类型；支持 deflate/bzip2/zstd |
| `async_zip` | 异步 ZIP | 0.0.19 | Modrinth App 在用；0.0.x API 不稳；大整合包解压可用 |
| `sha1` / `sha2` / `md-5` | 哈希 | 0.11 / 0.11 / 0.11 | RustCrypto，成熟；SHA1 用于 Mojang 资源，MD5 用于离线 UUID |
| `hex` / `base64` | 编码 | 0.4 / 0.23.1 | 成熟 |
| `directories` | 平台目录 | 6.0.0 | 2025-01 后更新少；**推荐 `dirs` 7.0.0**（更活跃） |
| `walkdir` | 递归遍历 | 2.5.0 | 稳定但更新慢；大目录可用 `jwalk` 0.9（并行） |
| `fs_extra` | 复制/移动目录 | 1.3.0 | **2023 后停更，不建议**；用 `std::fs` + `walkdir` 或 `fs_extra` 替代品 |
| `tempfile` | 临时文件 | 3.x | 下载临时分片 |
| `memmap2` | 大文件校验 | 0.9 | 可选；SHA1 大 jar 时减少拷贝 |

### B3.3 日志、错误、系统

| crate | 用途 | 版本 | 成熟度/备注 |
|---|---|---|---|
| `tracing` + `tracing-subscriber` + `tracing-appender` | 结构化日志/文件轮转 | 0.1.44 / 0.3 / 0.2 | 成熟；用 layer 做 token redaction |
| `thiserror` | 库错误枚举 | 2.0.21 | 成熟；领域层用 |
| `anyhow` | 应用层错误上下文 | 1.0.104 | 成熟；UI/命令层用 |
| `sysinfo` | CPU/内存/进程 | 0.39.6 | 成熟；内存推荐、进程监控 |
| `windows` | Win32 API | 0.62.2 | 官方；`EnumWindows`、`GetClassNameW`、`SetProcessWorkingSetSize`、`CreateMutexW` |
| `winreg` | 注册表 | 0.56.0 | Java 探测辅助；PCL 完全不用注册表，Rust 可作为补充 |
| `single-instance` | 单实例 | 0.3.3（2021） | **停更，不建议**；用 `interprocess` 或 `windows` 命名互斥体 |
| `interprocess` | 本地 socket/命名管道 | 2.4.6 | 活跃；单实例 IPC、第二实例传参 |
| `self_update` | 自动更新 | 1.3.1 | 活跃；GitHub/GitLab/S3、zipsign、SHA256、`self-replace` 2.3；Windows 自替换需 helper |
| `self-replace` | 运行中替换 exe | 1.5.0 | 稳定；可自研更新器时直接用 |
| `tray-icon` | 系统托盘 | 0.26.1 | Tauri 团队维护；Windows 需消息循环，Linux 需 libappindicator |
| `muda` | 菜单 | 0.17 | 与 tray-icon 配套 |
| `keyring` | 系统密钥库 | 4.2.0 | Windows Credential Manager / macOS Keychain / Linux Secret Service；Linux 无 DBus 时需 fallback |
| `notify-rust` | 桌面通知 | 4.18.2 | Linux DBus；Windows 用 `windows` toast 或 `tauri-plugin-notification` |
| `open` | 打开 URL/文件 | 5.4.4 | 成熟；登录页、日志目录 |
| `arboard` | 剪贴板 | 3.7.0 | 复制 device code、日志 |
| `secrecy` + `zeroize` | 敏感字符串封装/清零 | 0.10.3 / 1.9.1 | `SecretString` 不实现 `Debug`，配合 `zeroize` 防内存残留；`secrecy` 更新慢但稳定 |
| `fontdb` / `font-kit` | 系统字体枚举 | 0.24 / 0.14.3 | egui 系统字体 provider 已封装；自绘时可用 |

### B3.4 认证与 Minecraft 专用

| crate | 用途 | 版本 | 成熟度/备注 |
|---|---|---|---|
| `oauth2` | OAuth2/OIDC | 5.0.0 | 支持 Device Authorization Flow；但 MC 链路只需两个 POST，自研更可控 |
| `minecraft-auth` | MS/XBL/MC 全链路 | 0.1.1 | CCBlueX（LiquidBounce）出品，LGPL-3.0；模块 `msa/xbl/java`；**太新（下载 <1k），API 可能变**，可读源码参考 |
| `mc-launcher-core` | 安装/启动/loader | 0.1.2 | 模块全（auth/install/launcher/loader/forge），但 0.1.x、文档少 |
| `lighty-launcher` / `lighty-auth` | 启动器库/认证 | 26.9.3 | 模块化、更新勤；可作为 loader 安装参考，不建议直接依赖 |
| `uuid` | UUID | 1.28.0 | `v3/v4/v5` feature；离线 UUID 需手工复刻 Java 算法 |
| `semver` | 版本比较 | 1.0.28 | Java 版本不是严格 semver，需自定义比较 |

### B3.5 选型结论

- **必选**：tokio、reqwest(rustls + native-certs)、serde/serde_json、zip、sha1/sha2/md-5、tracing、thiserror/anyhow、tokio-util、bytes、directories/dirs、walkdir、sysinfo、keyring、interprocess、self_update（或自研）、tray-icon、open、arboard。
- **可选**：governor（限速）、async_zip（异步解压）、jwalk（并行遍历）、redb/rusqlite（实例/元数据缓存）、rust-i18n（i18n）、image（皮肤/图标）、notify（Mod 目录监视）。
- **避免**：`single-instance`（停更）、`fs_extra`（停更）、`simd-json`（收益低）、`sled`（alpha 多年）、`reqwest` 默认 native-tls（跨平台构建麻烦）。

**Java 检测的落地建议**：以**目录扫描为主、注册表为辅、实际执行校验为准**。PCL 的候选目录清单可直接借鉴（`PCLCS/Java.cs:136-170`）：官方启动器 runtime、HMCL、Prism、Modrinth App、CurseForge、`.jdks`、`.sdkman`、FTB、Microsoft Store 版 runtime、`Program Files\Java|Eclipse Adoptium|Amazon Corretto|Zulu|Microsoft\jdk-*`、`JAVA_HOME/JDK_HOME`。注册表（`HKLM\SOFTWARE\JavaSoft\...`）只作补充，因为绿色版/解压版通常不写。最终以 `java -XshowSettings:properties -version` 的输出解析 `java.version`、`java.vendor`、`sun.arch.data.model`（64 位）、`file.encoding`、`native.encoding`，并检查 `/lib/ext exists` 等坏安装特征。

---

## B4. 微软 Minecraft 认证在 Rust 中的可行路径

### B4.1 链路

```
device code (POST /consumers/oauth2/v2.0/devicecode)
   → 用户浏览器输入 code
   → 轮询 token (POST /consumers/oauth2/v2.0/token, grant_type=urn:ietf:params:oauth:grant-type:device_code)
   → MSA access_token + refresh_token
   → XBL (POST user.auth.xboxlive.com/user/authenticate, RpsTicket="d="+token)
   → XSTS (POST xsts.auth.xboxlive.com/xsts/authorize, RelyingParty=rp://api.minecraftservices.com/)
   → MC token (POST api.minecraftservices.com/authentication/login_with_xbox, identityToken="XBL3.0 x=<uhs>;<xsts>")
   → entitlement (GET /entitlements/mcstore)
   → profile (GET /minecraft/profile)
```

### B4.2 现成 crate 评估

| 方案 | 结论 |
|---|---|
| `oauth2` 5.0 的 `DeviceAuthorizationFlow` | 可用，帮你处理 device code 请求/轮询/错误；但 XBL/XSTS/MC 仍要自己写。适合想少写 OAuth 细节的团队 |
| `minecraft-auth` 0.1.1 | 覆盖 `msa/xbl/java`，可直接用；但 0.1.x、下载量极低、LGPL-3.0（静态链接需满足可重链接要求，闭源商用要谨慎）；建议**读源码 + 自研**，避免 API 锁定 |
| `lighty-auth` | 有 `microsoft/offline/azuriom` 模块，偏启动器框架；可参考错误处理 |
| 自研（推荐） | 链路只有 6 个 POST/GET，用 `reqwest` + `serde` 约 400–600 行；可完全控制错误码、重试、日志脱敏、token 存储 |

**推荐**：自研 `auth` 模块，内部可选 `oauth2` 只做 device code 两段；XBL/XSTS/MC 用强类型 `serde` 结构体。

### B4.3 必须注意的坑

1. **client_id 与 Azure 应用注册**
   - 必须是 **personal Microsoft accounts** 应用（`consumers` 租户），不能是 AAD 租户；scope 必须含 `XboxLive.signin` 与 `offline_access`。
   - 公共客户端（public client）需允许 device code flow；不需要 client secret。
   - **新注册的 Azure 应用调用 `api.minecraftservices.com` 需要向 Mojang 申请权限**（否则 403；Minecraft Wiki 的说明是“new created Azure Apps must apply”）。这是最大的合规/接入风险：要么申请，要么使用已获授权的 client id（不能盗用 PCL/官方启动器的）。
   - PCL 用环境变量注入 client id（`ModSecret.vb:10`），Rust 版用 `option_env!("MC_CLIENT_ID")` 或配置文件，仓库里只放占位符。
2. **tenant 只能是 `consumers`**，用 `common`/AAD tenant 会报错。
3. **RpsTicket 前缀**：MSA access token 必须加 `d=`；如果 token 已以 `d=` 开头不要重复加。
4. **XSTS 错误码**：2148916227 封禁、2148916233 无 Xbox 账户、2148916235 地区不支持、2148916238 未成年/需家长同意；要给出可操作的中文提示，而不是“登录失败”。
5. **`login_with_xbox` 的 429/403/503**：429 等待几分钟、403 关 VPN/换节点、503 Mojang 故障；不要盲目重试。
6. **entitlement 与 Game Pass**：`/entitlements/mcstore` 的 `items` 为空通常表示没有可用权益（含 XGP 到期），但也可能受 API 抖动影响；PCL 直接判失败，Rust 版可先查 profile，再给出“未检测到权益，仍可尝试启动”的选项，而不是硬阻断。
7. **profile 404**：账号没有创建 Minecraft 玩家档案，需要引导到 minecraft.net 创建；不是 token 失效。
8. **refresh token 轮换**：每次刷新都会返回新的 refresh_token，必须原子落盘；旧 token 立刻失效。多开/多进程要加锁。
9. **过期时间**：`expires_in` 是秒；PCL 提前 1 小时视为过期，避免边界失败。系统时钟偏差大时，用服务器 `Date` 头校正。
10. **TLS 信任**：reqwest 的 `rustls-tls` 默认用 webpki-roots，不读 Windows 证书 store；国内部分网络/企业代理会 MITM，导致 `CertificateVerifyFailed`。改用 `rustls-tls-native-roots` / 显式加载 `rustls-native-certs`，并提供“使用系统证书”开关；**不要**默认关闭校验。
11. **device code UX**：`verification_uri` 在部分网络打不开，要提供“复制链接/代码”、二维码、以及“用其他设备打开”的说明；轮询要支持取消。
12. **日志**：绝不打印 access/refresh token、`RpsTicket`、`identityToken`；错误响应体可能含 token 片段，落日志前要 redact。
13. **authlib-injector**：服务端根路径的元数据要预取并 base64 传给 `-Dauthlibinjector.yggdrasil.prefetched`，否则游戏启动时会再请求一次，弱网下可能超时；Java 6+ 加 `-Djavax.net.ssl.trustStoreType=WINDOWS-ROOT` 信任系统根证书。
14. **离线 UUID**：Java 的 `UUID.nameUUIDFromBytes` 是 MD5 + 设置 version 3/variant 位；不要用 `Uuid::new_v3` 的命名空间版本（结果不同），要逐字节复刻。

### B4.4 建议的 Rust 接口草图

```rust
type Result<T> = std::result::Result<T, AuthError>;
pub struct DeviceCodeFlow { client_id: String, http: reqwest::Client }
impl DeviceCodeFlow {
    pub async fn start(&self) -> Result<DeviceCode>; // user_code, verification_uri, device_code, interval, expires_in
    pub async fn poll(&self, dc: &DeviceCode, cancel: CancellationToken) -> Result<MsaToken>;
}
pub struct MsaToken { access_token: SecretString, refresh_token: SecretString, expires_at: SystemTime }
pub async fn xbl_authenticate(http: &Client, msa: &str) -> Result<String>;
pub async fn xsts_authorize(http: &Client, xbl: &str) -> Result<Xsts>;
pub struct Xsts { pub token: SecretString, pub uhs: String }
pub async fn mc_login(http: &Client, xsts: &Xsts) -> Result<McToken>;
pub async fn entitlements(http: &Client, mc: &str) -> Result<bool>;
pub async fn profile(http: &Client, mc: &str) -> Result<Profile>;
```

存储：`Account { id, username, uuid, kind: Microsoft|Offline|Authlib, refresh_token: Option<SecretString> }`；refresh token 存 `keyring`，账户元数据存 `accounts.json`（不含 secret）。

---

## B5. 打包与分发

### B5.1 Windows

| 形态 | 工具 | 优点 | 缺点 | 建议 |
|---|---|---|---|---|
| 单 exe（便携） | `cargo build --release --target x86_64-pc-windows-msvc` | 双击即用、无安装、易做“绿色版” | 无开始菜单/卸载/自动更新 | **主发布物**；配 `#![windows_subsystem = "windows"]` 去掉控制台 |
| Inno Setup | `iscc` + `.iss` | 脚本简单、压缩好、支持 per-user 安装（免 UAC）、代码签名 | 非 Rust 原生工具 | **推荐安装包**；比 MSI 更灵活 |
| WiX / MSI | `cargo-wix` | 企业部署、组策略、MSI 生态 | 学习曲线、WiX 4 变化、体积略大 | 需要企业分发时再做 |
| NSIS | Tauri/自研 | 体积小、Tauri 默认 | 脚本语言老、易被杀软误报 | 可选 |
| MSIX | `msix-packaging` / `MakeAppx` | 沙箱、自动更新、Store | **必须签名**、沙箱限制（写任意目录/启动 Java 受限） | 不适合启动器（需要写 `.minecraft`、启动子进程） |

- **静态 CRT**：`RUSTFLAGS="-C target-feature=+crt-static"`（MSVC）可去掉 `VCRUNTIME140.dll` 依赖，体积 +~200 KB；Win10/11 通常已有 UCRT，但静态链接更“免安装”。
- **签名**：Authenticode（EV/OV）能减少 SmartScreen 拦截；未签名的新 exe 首次运行会弹“Windows 已保护你的电脑”。签名后不要 UPX（会破坏签名/触发启发式）。
- **自动更新**：`self_update` + GitHub Releases 最简单；Windows 上运行中的 exe 不能覆盖，`self-replace` 会启动 helper 完成替换，或学 PCL：下载 `Latest.exe`，以 `--update <pid> <old> <new>` 启动自身副本，退出后替换。更新包必须签名/校验 SHA256，防降级/篡改。

### B5.2 macOS

- `cargo build --release --target aarch64-apple-darwin` + `x86_64-apple-darwin`，`lipo -create` 出 universal2（体积接近 2 倍）。
- `.app` 结构：`Contents/MacOS/<bin>`、`Contents/Info.plist`、`Contents/Resources/`；`cargo-bundle` 或 `tauri-bundler` 可生成。
- 分发：Developer ID 签名 + `notarytool` 公证 + staple；否则 Gatekeeper 拦截。DMG 用 `create-dmg`/`dmgbuild`。
- 注意：macOS 的 `.minecraft` 在 `~/Library/Application Support/minecraft`；Java 探测路径不同；`keyring` 用 Keychain。

### B5.3 Linux

- **AppImage**：单文件、免安装，但需要打包 GTK/GL 依赖；egui 依赖 `libxkbcommon`、`libGL`、`libwayland`（多数桌面已有）；WebKit 方案在 AppImage 里很容易缺库。
- **.deb**：`cargo-deb`，声明依赖（`libxkbcommon0`、`libgl1`），适合 apt 用户。
- **Flatpak**：沙箱需要额外权限（访问 `~/.minecraft`、启动 Java），不适合首发。
- glibc 基线：在旧发行版（如 Ubuntu 22.04）构建，避免 GLIBC_2.39 符号；或用 musl 静态链接（但 GPU/GL 的 dlopen 可能有问题，需测试）。

### B5.4 体积优化手段与取舍

| 手段 | 效果（经验） | 代价 | 建议 |
|---|---|---|---|
| `strip = true` | -30%~50% | 无符号调试 | **必开** |
| `lto = "fat"` | -10%~20% | 编译慢 | **必开** |
| `codegen-units = 1` | -5%~10% | 编译慢 | **必开** |
| `opt-level = "z"` | 比 `"s"` 再小 2%~5% | 可能更慢 | 体积优先用 `"z"`，性能敏感用 `"s"` |
| `panic = "abort"` | -5%~10% | 无 unwind，错误处理需注意 | **可开**（除非确实需要 `catch_unwind` 兜底） |
| `+crt-static` | +0.2 MB | 无 VC 运行时依赖 | Windows 推荐 |
| `build-std` + `panic_immediate_abort` | 再 -10%~20% | 需 nightly、工具链复杂 | 可选，收益递减 |
| UPX | -50%~70% | **杀软误报、内存解压、签名失效、启动变慢** | **不推荐** |
| 移除默认字体/图标 | 数 MB | 需自备 | 按需 |
| 字体子集化 | 10 MB → 2–4 MB | 缺字风险 | 中文用子集 + 系统回退 |

实测：egui hello 默认 release 8.67 MiB → 全优化 5.15 MiB（-41%）；iced tiny-skia 3.85 MiB；slint 9.72 MiB。**优化后 5–10 MB 是 Rust 原生启动器的合理区间**，与 PCL 的 5.7 MB 同量级。

---

## B6. WebView（Tauri/wry）vs 纯原生，在“轻量”目标下的取舍

### B6.1 WebView2 依赖事实

Tauri 官方 Windows 安装器选项（[Tauri docs](https://v2.tauri.app/distribute/windows-installer/)）：

| 模式 | 需联网 | 附加体积 | 说明 |
|---|---|---:|---|
| `downloadBootstrapper`（默认） | 是 | **0 MB** | 安装时下载引导器；Win7 MSI 不推荐 |
| `embedBootstrapper` | 是 | **~1.8 MB** | 引导器内置，Win7 更稳 |
| `offlineInstaller` | 否 | **~127 MB** | 内置完整 WebView2 安装器 |
| `fixedVersion` | 否 | **~180 MB** | 固定版本运行时，随应用分发 |
| `skip` | — | 0 MB | 不推荐，缺运行时直接白屏 |

- Windows 11 与“已更新的 Win10”通常自带 WebView2 Runtime；但 Win10 1803 以前、LTSC、精简版、部分企业镜像可能没有。**“下载一个 exe 双击即用”在 Tauri 上不成立**：要么联网装运行时，要么带 127/180 MB。
- WebView2 是共享运行时，安装后磁盘占用约 100–200 MB（系统级），不是每个应用一份；但**内存是多进程模型**，一个窗口常见 100–300 MB。
- 已知冷启动问题：WebView2 初始化 2–3 s 是 Windows 已知约束（tauri#10822）；以管理员运行时可能 >20 s（WebView2Feedback #5300 / tauri#13727）。
- 渲染一致性受 WebView2 版本影响；GPU 加速在远程桌面/虚拟机/老显卡上可能退回软件渲染，CPU 占用高。

### B6.2 原生方案

- **优点**：单 exe、无运行时、启动 20–100 ms、内存 30–100 MB、行为确定、可离线、可做像素级自绘；对“启动器”这种工具类 UI 足够。
- **缺点**：UI 开发效率低于 Web（没有 CSS/React 生态）；复杂动画/富文本要自己做；无障碍、输入法、DPI 需要逐项打磨。
- **结论**：nailong 的定位（单文件、无运行时、低内存、快启动）与 WebView 的核心代价直接冲突。**选原生（egui 或 iced），不选 Tauri/wry/Dioxus-WebView。**

---

# C. 对 nailong 的架构建议

## C.1 推荐技术栈（与现有 README 一致，但给出备选与边界）

| 层 | 选择 | 理由/备注 |
|---|---|---|
| GUI | **egui/eframe 0.37 + glow** | 5 MB、40 ms、开发快；系统字体 provider 解决中文；备选 iced 0.14 + tiny-skia |
| 异步 | tokio 1.54（multi-thread） | 下载/登录/安装并发；UI 线程只做渲染 |
| HTTP | reqwest 0.13 + rustls + **rustls-native-certs** | 必须信任系统根证书；避免 OpenSSL |
| JSON | serde + serde_json | 版本 JSON 小；不需要 simd-json |
| ZIP | zip 9（同步，spawn_blocking） | 解压大包放阻塞线程池；异步场景可评估 async_zip |
| 哈希 | sha1/sha2/md-5 | Mojang SHA1、离线 UUID MD5 |
| 下载 | 自研 Downloader（tokio + governor + CancellationToken） | 学 PCL：分片、源隔离、回退、断点、限速 |
| 日志 | tracing + tracing-subscriber + appender | token redaction layer |
| 错误 | thiserror（领域）+ anyhow（应用） | 可解释失败 |
| 配置 | TOML/JSON + `dirs` 7（不用停更的 `directories`） | 便携模式：exe 同级 `data/` |
| 密钥 | keyring 4.2 | refresh token/密码；失败回退加密文件 |
| 单实例 | interprocess 2.4（本地 socket） | 第二实例把参数传给主实例并退出；比 `single-instance` 可靠 |
| 更新 | self_update 1.3 + self-replace | GitHub Releases + 签名/SHA256；或自研 `--update` helper |
| 托盘 | tray-icon 0.26 + muda | 可选；Windows 消息循环 |
| Java 探测 | 自研（winreg 辅助 + 目录扫描 + `java -XshowSettings`） | 学 PCL 的候选目录列表；校验 64 位与编码 |
| i18n | rust-i18n 或自研 JSON | 中英优先 |

## C.2 模块划分（建议 workspace）

```
nailong/
├─ crates/
│  ├─ core/        版本模型、rules、继承合并、路径/实例、错误
│  ├─ auth/        MS device code、XBL/XSTS、离线、authlib-injector
│  ├─ download/    源选择、分片、校验、限速、断点、镜像
│  ├─ install/     原版/loader/Java/整合包安装计划
│  ├─ launch/      LaunchPlan、参数、natives、进程、日志 watcher
│  ├─ java/        Java 发现、版本解析、运行时下载
│  └─ gui/         egui 页面、状态、任务进度
└─ src/main.rs     单实例、日志、更新、启动 GUI
```

关键原则（呼应 PCL 的优点、避开其缺点）：

1. **Plan/Execute 分离**：先算出纯数据的 `LaunchPlan`（Java、JVM args、mainClass、game args、classpath、natives、cwd、env），再执行；计划可快照、可测试、可在 UI 展示“将要执行的命令”。
2. **加载器树 → 结构化并发**：用 `JoinSet` + `CancellationToken` + 权重进度，复刻 PCL 的 `LoaderCombo` 语义；不要用全局单例。
3. **规则引擎完整实现**：`os.name/version/arch`、`features`（demo、custom resolution、quick play）、`rules` 顺序覆盖；这是跨平台正确性的底线。
4. **下载器按 PCL 的“源”建模**：每个 URL 是一个 `Source`，有独立失败计数/禁用状态/超时；文件是 `DownloadTask`，可多源、多分片；全局 `Semaphore` 限并发，`governor` 限速。
5. **校验分层**：大小（快速）→ SHA1（下载后）→ ZIP 完整性（安装时）→ 启动前关键文件存在性；失败可修复而不是只报错。
6. **凭据最小化**：access token 只在内存；refresh token 进 keyring；日志 redaction 默认开启。
7. **可测量**：CI 记录二进制大小、冷启动、空闲 RSS（20 实例），回归报警；这与项目 00-overview 的指标一致。

## C.3 与 PCL 的功能对照（MVP 优先级）

| 能力 | PCL | nailong MVP | 备注 |
|---|---|---|---|
| 原版安装/启动 | ✅ | P0 | version JSON + assets + natives |
| 版本继承/隔离 | ✅ | P0 | 完整 rules；隔离策略可先“每实例独立目录” |
| 微软登录 | ✅ | P0 | device code + keyring |
| 离线登录 | ✅ | P0 | Java UUID 算法 |
| 镜像/多源 | ✅ | P0 | BMCLAPI + MCIM；源级故障隔离 |
| Forge/Fabric/NeoForge | ✅ | P1 | installer processors 是难点；可先 Fabric/Quilt |
| 整合包 | ✅ | P1 | Modrinth `.mrpack` 优先 |
| authlib-injector | ✅ | P1 | 预取元数据 + javaagent |
| 自动更新 | ✅ | P1 | 签名 + helper |
| 托盘/通知 | ✅ | P2 | |
| 联机/皮肤/音乐 | ✅ | 非目标 | 保持轻量 |

---

# D. 数据来源与参考链接

**PCL**

- 仓库：https://github.com/Meloong-Git/PCL
- 官网/下载：https://meloong.com/pcl
- 本地源码：`/tmp/ref/pcl`，HEAD `6b9a50d`，版本 2.99.0.0
- 关键文件：`Plain Craft Launcher 2/Modules/Minecraft/ModLaunch.vb`、`ModMinecraft.vb`、`ModDownload.vb`、`Modules/Base/ModNet.vb`、`ModLoader.vb`、`Application.xaml.vb`、`FormMain.xaml.vb`、`PCLCS/Java.cs`

**Rust 启动器**

- Modrinth App / Theseus：https://github.com/modrinth/code
- PandoraLauncher：https://github.com/Moulberry/PandoraLauncher
- QuantumLauncher：https://github.com/Mrmayman/quantumlauncher
- OneLauncher：https://github.com/Polyfrost/OneLauncher
- LiquidLauncher：https://github.com/CCBlueX/LiquidLauncher
- portablemc：https://github.com/theorzr/portablemc
- Nitrolaunch：https://github.com/Nitrolaunch/nitrolaunch
- conic launcher（Slint）：https://github.com/conic-apps/launcher
- HelixLauncher：https://github.com/HelixLauncher/HelixLauncher
- SwiftLauncher（iced）：https://github.com/swiftforyou/SwiftLauncher
- basalt-launcher：https://github.com/MegalithOfficial/basalt-launcher
- shard：https://github.com/Th0rgal/shard
- scl（已归档）：https://github.com/Steve-xmh/scl

**GUI**

- egui：https://github.com/emilk/egui；0.37 changelog（系统字体 provider、IME）：https://github.com/emilk/egui/blob/master/CHANGELOG.md
- iced：https://github.com/iced-rs/iced；0.14 changelog（IME、响应式渲染）：https://github.com/iced-rs/iced/blob/master/CHANGELOG.md
- Slint：https://github.com/slint-ui/slint
- Tauri：https://github.com/tauri-apps/tauri；WebView2 安装选项：https://v2.tauri.app/distribute/windows-installer/
- gpui：https://github.com/zed-industries/zed；crates.io `gpui` 0.2.2
- Dioxus：https://github.com/DioxusLabs/dioxus
- 15 框架内存实测（2026-08）：https://zenn.dev/mizugeeks/articles/1019cf2353d343
- egui 体积讨论：https://github.com/emilk/egui/discussions/1651

**认证**

- Microsoft device code：https://learn.microsoft.com/entra/identity-platform/v2-oauth2-device-code
- Minecraft 认证链路：https://minecraft.wiki/w/Microsoft_authentication
- `oauth2` crate：https://docs.rs/oauth2
- `minecraft-auth`（CCBlueX）：https://github.com/CCBlueX/minecraft-auth-rs
- `keyring`：https://docs.rs/keyring
- `self_update`：https://docs.rs/self_update
- `interprocess`：https://docs.rs/interprocess
- `tray-icon`：https://docs.rs/tray-icon

**镜像/生态**

- BMCLAPI：https://bmclapi.bangbang93.com
- MCIM：https://mod.mcimirror.top
- crates.io API：https://crates.io/api/v1/crates/{name}（版本/下载量，2026-10-20 抓取）
- GitHub API：repos/releases（星数、发布时间、资产大小，2026-10-20 抓取）

---

## 附录：实测环境与复现命令

```bash
# 工具链
rustup toolchain install stable   # 1.99.1
# 构建（egui 为例）
cargo build --release             # profile: opt-level="z", lto="fat", codegen-units=1, panic="abort", strip=true
# 启动/内存（Linux，Xvfb + llvmpipe）
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 ./target/release/egui_hello &
pid=$!
for i in $(seq 1 400); do DISPLAY=:99 xdotool search --name '^hello$' >/dev/null 2>&1 && break; sleep 0.01; done
grep VmRSS /proc/$pid/status
```

> 再次强调：上述数字是 Linux/软件渲染；Windows 真实 GPU 环境下 egui/iced/slint 的内存通常明显更低。发布物体积（PCL 5.70 MiB、Quantum 8.1 MiB、Liquid 8.0 MiB 等）是跨平台可比的硬数据。
