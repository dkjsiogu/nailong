# HMCL 启动器架构研究报告

> **研究对象**：`/tmp/ref/hmcl`（HMCL，Java 17 / JavaFX，Gradle 多模块），HEAD = `8827fd0 优化地区检测逻辑 (#7020)`
> **方法**：只读源码分析（未修改仓库），逐包通读 + 关键类全文精读
> **目的**：为用 Rust 从零实现轻量 Minecraft 启动器 `nailong` 提取可复用的架构与机制
> **说明**：本文所有结论都附有文件路径 / 类名 / 方法名作为证据；路径均相对于仓库根目录

---

## 0. 速览：10 条最关键的结论

| # | 结论 | 证据 |
|---|---|---|
| 1 | 三层模块：`HMCLBoot`（Java 8 引导，448 行）→ `HMCL`（JavaFX UI + 应用编排，98.7k 行）→ `HMCLCore`（领域内核，65.4k 行），依赖严格单向 | `settings.gradle.kts`、`HMCL/build.gradle.kts`、`HMCLBoot/build.gradle.kts` |
| 2 | HMCLCore 的内核分层是「util ← game/task/java/auth ← download ← modpack」，`launch` 只依赖 game/auth/util | `HMCLCore/src/main/java/org/jackhuang/hmcl/` 包结构 |
| 3 | 版本继承不是"运行时字符串拼接"，而是**不可变 manifest 的递归 merge**；新版又叠加了**按 priority 排序的 patch 合成**（`patches` 字段） | `game/DefaultGameRepositorySnapshot.java:resolve()`、`game/GameInstanceManifest.java:merge()`、`game/GameInstancePatch.java` |
| 4 | "版本隔离"不是数据模型的一部分，而是**运行目录（run directory）选择策略**：默认共享 `.minecraft` 根，隔离=实例根，另有 per-instance `runningDirectory` 覆盖 | `game/GameInstance.java:getRunDirectory()`、`HMCL/game/HMCLGameRepositoryLayout.java`、`HMCL/game/HMCLGameRepository.java:computeRunDirectory()` |
| 5 | 启动器命令行是**分层拼装 + 最后一层变量替换**：程序生成参数 → manifest 的 `arguments.jvm` → 账号注入参数 → mainClass → 游戏参数 → 用户参数 | `launch/DefaultLauncher.java:generateCommandLine()`（方法本体约 360 行） |
| 6 | 下载体系是「候选 URL 列表（candidates）+ 全局并发闸门 + 内容寻址缓存」；镜像适配=**URL 前缀重写规则表**，不是接口实现 | `download/DownloadProvider.java`、`download/DownloadCandidates.java`、`task/FetchTask.java`、`HMCL/download/HMCLDownloadProvider.java` |
| 7 | 任务模型是自研的「声明式 DAG + 运行时动态扩展」：`getDependents()/getDependencies()` 双向、`execute()` 中可再追加任务 | `task/Task.java`（1296 行）、`task/AsyncTaskExecutor.java` |
| 8 | 加载器安装被抽象成「产出 `GameInstancePatch` 的任务」，Forge/NeoForge 的 installer processors 是唯一需要"跑外部 JVM"的重活 | `download/forge/ForgeNewInstallTask.java`、`download/fabric/FabricInstallTask.java`、`download/optifine/OptiFineInstallTask.java` |
| 9 | 账户层把「可公开元数据」与「私密凭据」分文件存储；微软登录支持 PKCE 授权码（localhost 回调）与设备码两条流程 | `auth/Account.java:writeMetadata/writePrivateData`、`auth/OAuth.java`、`HMCL/game/OAuthServer.java` |
| 10 | HMCL 的"重"来自 JavaFX + 自带 JavaFX 修补 + Forge/OptiFine 的历史包袱；但它有 5 个机制非常值得 Rust 照抄（§8.3） | `HMCL/EntryPoint.java:checkJavaFX()`、`util/SelfDependencyPatcher.java` |

---

## 1. 模块划分与依赖方向

### 1.1 三个构建模块

`settings.gradle.kts` 只 include 三个模块（外加两个嵌入 jar 的小模块 `HMCLTransformerDiscoveryService`、`HMCLMultiMCBootstrap`，它们只被打进 `assets/game/`）：

```kotlin
include("HMCL", "HMCLCore", "HMCLBoot")
```

| 模块 | 主源码规模 | 文件数 | 编译目标 | 职责 | 关键证据 |
|---|---|---|---|---|---|
| `HMCLBoot` | 448 行 | 6 | **Java 8**（`tasks.compileJava { options.release.set(8) }`） | 只做"能不能启动"的守卫：检查 `java.version >= 17`、路径合法性、弹 Swing 错误框，然后 `EntryPoint.main()` | `HMCLBoot/src/main/java/org/jackhuang/hmcl/Main.java`、`HMCLBoot/build.gradle.kts` |
| `HMCLCore` | 65 383 行 | 422 | Java 17 | 与 UI 无关的领域内核：版本模型、下载、安装、启动、账号、整合包、任务引擎 | `HMCLCore/build.gradle.kts` |
| `HMCL` | 98 747 行 | 443（其中 `ui/` 243） | Java 17 | JavaFX 应用：界面、设置、实例管理、启动编排、崩溃窗口、自更新 | `HMCL/build.gradle.kts`（shadowJar，`Main-Class = org.jackhuang.hmcl.Main`） |

**值得注意的引导链设计（Rust 可直接借鉴其思想）**：

1. 发布的 `HMCL-x.y.z.exe` = `HMCLauncher.exe` 二进制 + 整个 jar 拼接（`HMCL/build.gradle.kts` 的 `makeExecutables`：从 jar 里取出 `assets/HMCLauncher.exe`，后面追加 jar 字节）。
2. 原生 exe 负责找到/安装合适的 JRE 并 `java -jar`，jar 的 `Main-Class` 是 `org.jackhuang.hmcl.Main`（在 HMCLBoot）。
3. `Main` 用 Java 8 语法写成，只做版本判断；版本够就调 `EntryPoint.main`。
4. **`org.jackhuang.hmcl.EntryPoint` 在 HMCLBoot 里是"同名占位类"**（注释原文：*This is a dummy class and will be overwritten in the shadow jar*），真正的实现在 `HMCL/src/main/java/org/jackhuang/hmcl/EntryPoint.java`，shadowJar 时覆盖。
5. 真正的 `EntryPoint` 做：建目录 → 起日志 → 检查 Wine → macOS 特效 → `checkJavaFX()`（缺 JavaFX 就**现场下载并 patch 进 JVM**，`util/SelfDependencyPatcher.java` + `util/JavaFXPatcher.java`）→ `verifyJavaFX()` → `addEnableNativeAccess()` → `Launcher.main()`。

> 含义：HMCL 为了"一个 jar 到处跑"，付出了「自带 JavaFX 运行时补丁」的复杂度。Rust 单二进制天然没有这个问题——这是 nailong 最大的结构性优势。

### 1.2 HMCLCore 的包结构与分层

```
                        ┌──────────────────────────────────────────┐
                        │  modpack (44)  → download, game, task    │
                        └──────────────────────────────────────────┘
                                        ▲
                        ┌──────────────────────────────────────────┐
                        │  download (69) → game, task, java, addon │
                        │    ├ game/  forge/ fabric/ quilt/ neoforge/
                        │    ├ optifine/ liteloader/ cleanroom/
                        │    └ java/{mojang,disco}
                        └──────────────────────────────────────────┘
                                        ▲
        ┌───────────────────────────────────────────────────────────┐
        │  launch (10)   → game, auth, util        java (3) → util   │
        │  game (57)     → util, addon.mod, schematic                │
        │  task (13)     → event, util                               │
        │  auth (48)     → util, game                                │
        └───────────────────────────────────────────────────────────┘
                                        ▲
                        ┌──────────────────────────────────────────┐
                        │  util (175) / event (7) / addon (30)      │
                        └──────────────────────────────────────────┘
```

| 包 | 主源码文件数 | 职责 | 代表类 |
|---|---|---|---|
| `util` | 175 | 基础设施：IO/HTTP/压缩/JSON/平台/版本号/日志/JavaFX 小工具 | `util/io/HttpRequest.java`、`util/CacheRepository.java`、`util/platform/ManagedProcess.java`、`util/versioning/GameVersionNumber.java` |
| `download` | 69 | 远端版本列表、依赖管理、各类安装任务、缓存仓库 | `download/DownloadProvider.java`、`download/DefaultDependencyManager.java`、`download/DefaultGameBuilder.java`、`download/DefaultCacheRepository.java` |
| `game` | 57 | 版本/清单/库/资源/规则/布局/仓库快照/组件识别 | `game/GameInstanceManifest.java`、`game/DefaultGameRepository.java`、`game/Library.java`、`game/GameInstancePatch.java` |
| `auth` | 48 | 账户抽象、微软 OAuth、Yggdrasil、authlib-injector、离线 | `auth/Account.java`、`auth/OAuth.java`、`auth/microsoft/MicrosoftService.java` |
| `modpack` | 44 | 6 种整合包格式的读写/安装/补全 | `modpack/ModpackProvider.java`、`modpack/curse/CurseInstallTask.java` |
| `addon` | 30 | 模组/资源包/数据包元数据与远程仓库（Modrinth/CurseForge） | `addon/mod/ModManager.java`、`addon/repository/ModrinthRemoteAddonRepository.java` |
| `task` | 13 | 任务引擎 | `task/Task.java`、`task/AsyncTaskExecutor.java`、`task/FetchTask.java` |
| `launch` | 10 | 命令行生成、natives 解压、进程监控、启动脚本 | `launch/Launcher.java`、`launch/DefaultLauncher.java`、`launch/ExitWaiter.java` |
| `schematic` | 7 | 投影文件（Litematica 等）解析——与启动无关的附加功能 | `schematic/` |
| `event` | 7 | 全局事件总线（JVM 启动失败/异常退出/进程停止） | `event/EventBus.java`、`event/ProcessExitedAbnormallyEvent.java` |
| `java` | 3 | Java 运行时值对象与仓库接口 | `java/JavaRuntime.java`、`java/JavaInfo.java`、`java/JavaRepository.java` |

### 1.3 依赖方向如何被约束（以及约束的漏洞）

**被约束的部分（有效）**：
- Gradle 依赖单向：`HMCL` → `HMCLCore`，`HMCL` → `HMCLBoot`；`HMCLCore` 的 `build.gradle.kts` **没有** 对 `HMCL` 的依赖。
- 实测 `HMCLCore/**` 中对 `org.jackhuang.hmcl.{ui,setting,Metadata}` 的 import 数为 **0**。
- HMCLCore 的 `api(...)` 列表就是它的能力边界：gson、tomlj、xz、lz4、nanohttpd、jsoup、JNA、kala-compress、commonmark、hello-nbt 等——**没有任何 UI 框架的"应用层"依赖**（除 JavaFX 基础库，见下）。

**漏洞（重要）**：
- `HMCLCore` 的 422 个文件中有 **35 个 import 了 `javafx.*`**，例如 `task/Task.java` 用 `javafx.beans.property.DoubleProperty` 做进度、`game/DefaultGameRepository.java` 用 `ObjectProperty` 发布快照、`auth/Account.java` 实现 `javafx.beans.Observable`。也就是说 **core 并不是纯粹的 domain，它绑定了 JavaFX 的 observable/属性模型**。
- 没有 `module-info.java`，没有 ArchUnit/依赖测试；checkstyle 只检查 `RedundantImport/UnusedImports/IllegalImport`（`config/checkstyle/checkstyle.xml`）。约束纯靠约定 + 代码评审。

> **对 nailong**：core 一旦绑 UI 框架的 observable，就再也回不到"纯库"状态。Rust 侧应让 `nailong-core` **零 GUI 依赖**，进度/事件用 `tokio::sync::watch` / `crossbeam` channel / 自定义 `ProgressSink` trait 表达，由 UI crate 适配（详见 §5.5）。

### 1.4 建议的 crate 映射

```
nailong/
├─ crates/
│  ├─ nl-util/        # 路径、摘要、压缩解压、HTTP 客户端、平台探测（对应 util）
│  ├─ nl-version/     # manifest 模型 + 继承/patch 合成 + 组件识别（对应 game）
│  ├─ nl-task/        # 任务 DAG / 进度 / 取消（对应 task）
│  ├─ nl-download/    # provider、candidates、fetch、cache（对应 download）
│  ├─ nl-install/     # 各 loader / java runtime 安装器（对应 download.* 子包）
│  ├─ nl-launch/      # 命令行、natives、进程与日志（对应 launch）
│  ├─ nl-auth/        # 账户（对应 auth）
│  ├─ nl-modpack/     # 整合包（对应 modpack）
│  └─ nl-core/        # 门面：GameRepository / DependencyManager 等价物
├─ apps/
│  ├─ nailong-cli/    # 无头：install / launch / export script
│  └─ nailong-gui/    # 轻量 GUI（egui/slint/tauri 任选）
```

---

## 2. 版本与实例

### 2.1 核心数据模型

`game/GameInstanceManifest.java` 是一个 **Java record（不可变）**，字段与官方 version JSON 一一对应，外加 HMCL 扩展：

```java
public record GameInstanceManifest(
    GameInstanceID id, String minecraftArguments, Arguments arguments, String mainClass,
    GameInstanceID inheritsFrom, GameInstanceID jar,
    AssetIndexInfo assetIndex, String assets, Integer complianceLevel, GameJavaVersion javaVersion,
    List<Library> libraries, List<CompatibilityRule> compatibilityRules,
    Map<DownloadType, DownloadInfo> downloads, Map<DownloadType, LoggingInfo> logging,
    ReleaseType type, Instant time, Instant releaseTime, Integer minimumLauncherVersion,
    Boolean root, Boolean hidden,                 // ← HMCL 扩展
    List<GameInstancePatch> patches,              // ← HMCL 扩展（核心）
    JsonObject rawJson                            // ← 原样保留未知字段
)
```

其他模型（均在 `HMCLCore/src/main/java/org/jackhuang/hmcl/game/`）：

| 类 | 关键点 |
|---|---|
| `Artifact` | Maven 坐标 `group:name:version[:classifier][@ext]`，构造时预算好 `path`（`a/b/c/name-ver-class.jar`）与 `fileName`，避免运行期反复拼串 |
| `Library` | `artifact + url + downloads{artifact,classifiers} + checksums + extract + natives + rules + hint + filename`；`natives` 支持 `[native-]os[-arch]` 多写法（`POSSIBLE_NATIVE_DESCRIPTORS`）；`hint == "local"` 表示库放在实例私有 `libraries/`（MultiMC 兼容）；`getDownload()` 会把 `url` 兜底成 `${url or DEFAULT_LIBRARY_URL}/${path}` |
| `AssetIndex` / `AssetObject` | `virtual` / `map_to_resources` 两个标志决定是否要重建 `assets/virtual/<id>/` 与 `resources/`（`DefaultGameInstance.reconstructAssets()`） |
| `Arguments` | 只有 `game` / `jvm` 两个 `List<Argument>`；`Argument` 是 sealed 风格接口，实现为 `StringArgument` 与 `RuledArgument`（带 `CompatibilityRule`） |
| `CompatibilityRule` | `action(ALLOW/DISALLOW) + os(name/version/arch) + features{}`；求值语义是**"最后一个命中的规则生效"**，默认 `DISALLOW` |
| `ExtractRules` | 只有 `exclude` 前缀列表，`shouldExtract(path)` 做前缀匹配 |
| `GameJavaVersion` | `component + majorVersion`，内置 `JAVA_8/16/17/21/25` 五个官方 runtime 组件名（`jre-legacy`、`java-runtime-alpha/beta/delta/epsilon`） |
| `LaunchOptions` | 启动参数聚合对象（525 行），见 §3.1 |

### 2.2 版本继承：递归 merge，不是字符串拼接

入口是 `game/DefaultGameRepositorySnapshot.java:resolve(GameInstanceManifest)`：

```java
if (manifest.inheritsFrom() == null) { ... }
else {
    if (!resolvedSoFar.add(manifest.id())) {      // 环检测
        LOG.warning("Found circular dependency instances: " + resolvedSoFar);
        resolvedManifest = manifest.withJar(...).withInheritsFrom(null);
    } else {
        parentResolved = resolve(parent.getManifest(), resolvedSoFar);
        resolvedManifest = manifest.merge(parentResolved);   // ← 子先父后
    }
}
```

`GameInstanceManifest.merge(parent)` 的合并规则（**子优先，列表拼接**）：

| 字段 | 规则 |
|---|---|
| `minecraftArguments` / `mainClass` / `jar` / `assetIndex` / `assets` / `javaVersion` / `downloads` / `logging` / `type` | 子为 null 才用父 |
| `arguments` | `Arguments.merge(parent, child)`：game 列表 **父在前子在后**，jvm 同理（`Lang.merge` 只是 `addAll`） |
| `libraries` | `Lang.merge(this.libraries, parent.libraries)` → **子库在前，父库在后** |
| `compatibilityRules` | `Lang.merge(parent, child)` → 父在前 |
| `minimumLauncherVersion` | 取 `Math::max` |
| `patches` | `parent.patches + [toPatch()] + child.patches`（把自身也固化成一个 hidden patch） |
| `inheritsFrom` | 置 null（已解析） |

> 注意 `libraries` 的顺序：子在前意味着 **子版本可以"遮蔽"父版本的库**（classpath 是 `LinkedHashSet`，先到先得，见 `DefaultLauncher.getClasspath()`）。Forge/OptiFine 安装器正是依赖这一点来覆盖原版库。

### 2.3 patch 机制：把"安装"变成"合成"

这是 HMCL 近期最大的模型演进，也是**对 Rust 实现最有价值的设计之一**。

- 每个组件（原版、Forge、Fabric、OptiFine、LiteLoader……）安装后不再"改写 version JSON"，而是产出一个 `GameInstancePatch`：
  ```java
  record GameInstancePatch(id, version, priority,
      minecraftArguments, arguments, mainClass, inheritsFrom, jar, assetIndex, assets,
      complianceLevel, javaVersion, libraries, compatibilityRules, downloads, logging,
      type, time, releaseTime, minimumLauncherVersion, hidden, rawJson)
  ```
- 优先级常量：`PRIORITY_MC = 0`、`PRIORITY_LOADER = 30000`；具体安装器还会自定：OptiFine `10000`、LiteLoader `60000`。
  → **数值即叠加顺序**：MC(0) → OptiFine(10000) → Forge/NeoForge/Cleanroom(30000) → LiteLoader(60000)。
- 合成：`DefaultGameRepositorySnapshot.resolve()` 末尾
  ```java
  manifest.patches().stream().sorted(comparing(GameInstancePatch::getPriority))
      .forEach(builder::merge);
  ```
- 单独重算：`GameInstanceManifest.reconstructByPatches()`（只对 root 且无 inheritsFrom 的实例有效）。
- 移除组件：`removeComponent(type)` = 去掉对应 patch 再重建。
- `GameInstancePatch.fromManifest(...)` 让"任意 manifest"都能变成一个 patch —— 这是 `GameInstallTask`、`FabricInstallTask` 等统一返回 `Task<GameInstancePatch>` 的基础。

**落盘形态**（`GameInstanceManifest.toJsonObject()`）：一个 root 实例的 `versions/<id>/<id>.json` 同时包含
① 已解析出来的 `libraries/arguments/mainClass/...`（给别的启动器看，保证互操作）
② `patches: [...]`（HMCL 的真相来源）
③ `root: true`。

解析时 `resolve()` 对 `isRoot() && patches != null` 的实例会**丢弃 ①**，只用 ② 重建：

```java
resolvedManifest = manifest.patches() != null
        ? new GameInstanceManifest(manifest.id()).withPatches(manifest.patches())
        : manifest;
```

> **这是个"双写"设计**：兼容性与可组合性兼得，代价是两套数据可能不一致，且 resolve 结果必须缓存（`DefaultGameInstance.resolvedManifest`）。
> Rust 侧建议：**只信 patches**，把"派生出的扁平 manifest"当作可随时重建的缓存（`#[serde(skip)]` + `OnceCell`），不要读回。

### 2.4 版本隔离与游戏目录

**关键抽象**：`game/GameRepositoryLayout.java`（纯路径计算，无 IO）

```java
Path getBaseDirectory();                       // .minecraft
Path getInstanceRoot(GameInstanceID id);       // versions/<id>
Path getLibrariesDirectory();                  // libraries
Path getLibraryFile(owner, library);           // 支持 hint=local → versions/<id>/libraries/...
Path getAssetDirectory();                      // assets
Path getAssetIndexFile(assetId);               // assets/indexes/<id>.json
Path getAssetObject(AssetObject);              // assets/objects/<xx>/<hash>
Path getLoggingObject(assetId, LoggingInfo);   // assets/log_configs/<file>
```

实现：`DefaultGameRepositoryLayout`（官方布局）；HMCL 扩展 `HMCL/game/HMCLGameRepositoryLayout.java` 只加了实例元数据目录：
```
versions/<id>/.hmcl/config/instance-game-settings.json
versions/<id>/.hmcl/state/
```

**"版本隔离"的真相**（`game/GameInstance.java:getRunDirectory()` + `HMCL/game/HMCLGameRepository.java:computeRunDirectory()`）：

```java
Path computeRunDirectory(instanceId, modpack, localSetting) {
    Path instanceRoot = layout.getInstanceRoot(instanceId);
    if (modpack) return instanceRoot;                       // 整合包永远隔离
    boolean useInstanceRunningDirectory =
        localSetting != null && localSetting.getOverrideProperties().contains("runningDirectory");
    String runningDirectory = selectedRunningDirectory(localSetting, useInstanceRunningDirectory);
    if (isBlank(runningDirectory))
        return useInstanceRunningDirectory ? instanceRoot : getBaseDirectory();   // 默认共享
    return Path.of(runningDirectory);                        // 用户自定义
}
```

三层来源：**实例本地设置（`runningDirectory` 覆盖项）→ 预设（Preset）继承值 → 全局默认**；`enableIsolation()` 只是往本地设置的 `overrideProperties` 里加一个 key（`HMCL/game/HMCLGameInstance.java:219`）。安装新实例时的默认策略是枚举 `DefaultIsolationType { ALWAYS, MODDED, NEVER }`（`HMCL/setting/DefaultIsolationType.java`，默认 `MODDED`）。

> **对 Rust 的启示**：不要做 `is_isolated: bool` 字段。做 `RunDirectory = Shared | InstanceRoot | Custom(PathBuf)` 的**解析函数**，输入是（实例、整合包标志、实例设置、预设、全局设置）。这样"隔离"就是配置解析的副产品，能无损支持迁移与自定义目录。

### 2.5 HMCL 的扩展字段/文件清单（互操作时必须容忍）

| 位置 | 字段/文件 | 语义 |
|---|---|---|
| version JSON | `root: bool` | 该实例是"patch 合成型"根实例 |
| version JSON | `hidden: bool` | UI 中隐藏 |
| version JSON | `patches: [{id, version, priority, hidden, ...manifest 字段}]` | 组件合成链 |
| version JSON | `jar: <id>` | 主 jar 复用另一版本（继承时常用） |
| `versions/<id>/` | `modpack.json`（HMCL）/`modpack.cfg`（Default 布局默认值） | 整合包配置（`ModpackConfiguration`） |
| `versions/<id>/.hmcl/config/instance-game-settings.json` | — | 实例级设置（带 `schema` 版本号） |
| 资源 | `assets/game/unlisted-versions.json`、`version-alias.csv`、`versions.txt` | 补全 Mojang 清单里没有的版本、版本别名、快照排序表（`GameRemoteVersion`、`GameVersionNumber`） |

---

## 3. 启动流水线

### 3.1 总览：谁在编排

`HMCLCore` 只提供**无状态的命令构造器**（`launch/Launcher` 抽象 + `DefaultLauncher` 实现）；**编排在 HMCL 模块**（`HMCL/game/LauncherHelper.java`，1079 行）。

`LauncherHelper.launch0()` 用任务链串起整条流水线（该方法本身就是最好的"启动流程文档"）：

```
checkGameState(instance, setting, manifest)          // 选 Java：设置 → 版本 JSON → 约束 → 自动下载
  → NativePatcher.patchNative(...)                   // 按平台改写 natives 库集合 / 移除 lwjgl natives
  → dependencyManager.checkGameCompletionAsync(...)  // 补全 client jar / libraries / assets
     + modpack provider.createCompletionTask(...)    // 整合包补全（Curse/Modrinth/…）
     + Mesa/LWJGL agent 下载
  → GameVerificationFixTask                          // 修复老版本资源索引
  → logIn(account)                                   // 登录（可能弹窗）
  → new LaunchOptions.Builder()...create()           // 汇总所有开关
  → new HMCLGameLauncher(instance, launchManifest, authInfo, options, listener)
  → launcher.launch()  |  launcher.makeLaunchScript(scriptFile)
  → 注册进程弱引用 / 关闭启动器 / 绑定"取消=杀进程"
```

各阶段通过 `withStage("launch.state.dependencies")` 等标记，配合 `withStagesHints(...)` 给 UI 显示进度分组——这是一个**"阶段名 + 计数"驱动进度**的轻量方案，值得照抄（见 §5.2 `withCounter`）。

`LaunchOptions`（`game/LaunchOptions.java`）是这条链的"参数总线"，字段可分 5 组：

| 组 | 字段 |
|---|---|
| 身份 | `instanceId, versionName, versionType, profileName, gameDir` |
| Java | `java, javaExecutable, minMemory, maxMemory, metaspace, javaArguments, overrideJavaArguments, javaAgents, noGeneratedJVMArgs, noGeneratedOptimizingJVMArgs` |
| 游戏 | `width, height, fullscreen, quickPlayOption, gameArguments, disableAutoGameOptions` |
| 进程 | `wrapper, proxyOption, processPriority, environmentVariables, preLaunchCommand, postExitCommand, daemon` |
| 图形/natives | `renderer, graphicsBackend, useCustomNatives, nativesDir, useNativeGLFW, useNativeOpenAL, useHighPerformanceGPU, enableDebugLogOutput, allowAutoAgent` |

### 3.2 命令行拼装顺序（`DefaultLauncher.generateCommandLine()`）

这是一份可以直接翻译成 Rust 的"顺序规格"，顺序本身就是语义（后写的覆盖先写的）：

1. **进程优先级前缀**：Linux/macOS 用 `nice -n -5/-1/1/5`；Windows 留空（注释里保留了 `cmd /C start /high` 方案）。
2. **wrapper**（`options.getWrapper()`，tokenize 后原样放入）。
3. **java 可执行文件**。
4. **用户完全覆盖的 JVM 参数**（`overrideJavaArguments`，`addAllWithoutParsingAndReadExternal` —— 不再做变量替换、支持 `@argfile`）。
5. **内存**：`-Xmx<n>m`、`-Xms<n>m`（仅当 `min <= max`）。
6. **Metaspace**：Java 8 以下用 `-XX:PermSize=`，否则 `-XX:MetaspaceSize=`。
7. **默认 JVM 参数集合**（`javaArguments`）。
8. **编码**：`-Dfile.encoding=<NATIVE_CHARSET>`，随后据此改 `sun.stdout/stderr.encoding`（<19）或 `stdout/stderr.encoding`（>=19）。
9. **Log4j2 RCE 缓解**：`-Djava.rmi.server.useCodebaseOnly=true`、`-Dcom.sun.jndi.rmi.object.trustURLCodebase=false`、`-Dcom.sun.jndi.cosnaming.object.trustURLCodebase=false`、`-Dlog4j2.formatMsgNoLookups=true`。
10. **`-Dlog4j.configurationFile=`**（当 `isUsingLog4j()` 且（debug 或用户没显式关掉））。
11. **"不生成 JVM 参数"开关关闭时**（`!noGeneratedJVMArgs`）：
    - `appendJvmArgs()`（子类钩子；HMCL 在这里加 `-javaagent:lwjgl-unsafe-agent`）
    - `-Dminecraft.client.jar=<abs>`
    - macOS `-Xdock:name/-Xdock:icon`
    - 非 Windows `-Duser.home=<gameDir 的父目录>`（**沙盒化 home，避免游戏污染真实 home**）
    - 代理：`-Djava.net.useSystemProxies=true` 或 http/socks 全套
    - **自动优化参数**（`!noGeneratedOptimizingJVMArgs`）：`UnlockExperimentalVMOptions`、G1GC 一组（`G1MixedGCCountTarget=5` 等）、JIT 参数（内存 >4G 且 64 位时 `ReservedCodeCacheSize=400M` 等）、32 位补 `-Xss1m`、Java 25/26 开 `UseCompactObjectHeaders`
    - `-Dfml.ignoreInvalidMinecraftCertificates/-Dfml.ignorePatchDiscrepancies`
12. **版本相关开关**：Java 16 `--illegal-access=permit`；Java 24/25 `--sun-misc-unsafe-memory-access=allow`。
13. **渲染后端环境**（Mesa/Vulkan ICD 的 `-D`，见 §3.4）。
14. **manifest 的 `arguments.jvm`**（若为 null 则用 `Arguments.DEFAULT_JVM_ARGUMENTS`）——这里才是 `-cp ${classpath}`、`-Djava.library.path=${natives_directory}`、`-Dminecraft.launcher.brand/version` 的来源。
15. **账号注入的 JVM 参数**（`authInfo.getLaunchArguments(options).jvm()`，authlib-injector 走这里）。
16. **`-javaagent:`**（`options.getJavaAgents()`）。
17. **mainClass**。
18. **旧版 `minecraftArguments`**（tokenize 后逐段替换）→ 再补 `DEFAULT_GAME_ARGUMENTS`（含 `--width/--height` 的 ruled 参数）。
19. **`arguments.game`**（带 `features` map）。
20. **账号注入的游戏参数**。
21. **QuickPlay**：`--quickPlayMultiplayer` / `--quickPlaySingleplayer` / `--quickPlayRealms`；老版本降级为 `--server/--port`。
22. `--fullscreen`、SOCKS 的 `--proxyHost/--proxyPort/--proxyUser/--proxyPass`、`--graphicsBackend`。
23. **用户自定义游戏参数**（`options.getGameArguments()`）。
24. **最后过滤** `forbiddens`（如 Java>=9 时删除 `-Xincgc`）。

> **"默认参数"的语义（很容易被忽略但很关键）**：上面大量使用 `CommandBuilder.addDefault(opt, value)`——它会先扫描已有参数（含 `@argfile` 外部参数），**只要存在同前缀参数就整体放弃**（`util/platform/CommandBuilder.java:196`，日志会打印 "Default option ... is suppressed by ..."）。
> 因此 HMCL 不需要"先加后删"的冲突消解：**所有自动生成的参数都是"可被用户覆盖的默认值"**，`-Xmx`、`-Dlog4j2.formatMsgNoLookups`、G1 参数、`-Duser.home` 全部遵循这一规则。Rust 实现若也采用"默认值注入器"，能省掉大量 if-else。

**变量替换**发生在 `Arguments.parseArguments(args, configuration, features)`，机制极简（`game/StringArgument.java`）：

```java
Pattern.compile("\\$\\{(.*?)}")   // 逐个 ${...} 用 map 替换，找不到就保留原样
```

### 3.3 classpath 构成（`DefaultLauncher.getClasspath()`）

```
classpath = [] (LinkedHashSet，去重且保序)
for lib in manifest.libraries:
    if !lib.appliesToCurrentEnvironment(): continue      # os/arch/features 规则
    if lib.isNative(): continue                           # natives 不进 classpath
    # OptiFine 特殊处理（仅当同时存在 Forge/LiteLoader）：
    #   - 优先用 OptiFine 的 *installer* jar 代替 patch jar
    #   - 丢弃 optifine:launchwrapper-of（Forge/LiteLoader 自带）
    if file exists: classpath.add(abs(file))
if optifineInstaller 且 mainClass != ModLauncher: classpath.add(installer)   # 放最后
classpath.add(instance jar)                              # 最后是主 jar
```

要点：
- **"文件存在才加入"**：classpath 由磁盘状态决定，而不是清单决定——这让"缺库也能启动"成为可能（代价是错误后置到游戏崩溃）。
- 主 jar 由 `manifest.jar()` 决定（`DefaultGameInstance.getInstanceJarFile()`），支持 `jar` 重定向。
- Cleanroom 会删掉几条冲突库（`2.9.4-nightly-20150209`、`platform-3.4.0`、`icu4j-core-mojang`）。

### 3.4 natives 处理

1. **目录**：`getNativeFolder()` = `options.nativesDir` 或 `instance.getNativeDirectory(platform)` = `versions/<id>/natives-<platform>`（`GameInstance.java` 默认方法）。**注意是实例根，不是运行目录**——即使不隔离，natives 也天然按实例分开。
2. **解压**：`decompressNatives(dest)` 对每个 `library.isNative()` 的库，用 `util/io/Unzipper` 解压到目标目录，过滤器逻辑：
   - 已存在且大小相同 → 跳过（幂等、可增量）
   - 跳过 `.sha1` / `.git`
   - `useNativeGLFWorSDL` 时跳过文件名含 `glfw` / `sdl` 的；`useNativeOpenAL` 时跳过 `openal`
   - 最后套 `library.getExtract().shouldExtract(relativePath)`（`exclude` 前缀）
   - `setReplaceExistentFile(false)`
3. **非 ASCII 路径 workaround**：Linux/macOS 且 natives 路径含非 ASCII 且 MC < 1.19 时，创建 `/tmp/hmcl-natives-<uuid>` 符号链接并把 `${natives_directory}` 拼成 `"/tmp/xxx:<real>"`（lwjgl 只接受 ASCII 路径）。
4. **`-Djava.library.path` 的子目录探测**：若 manifest 里写了 `-Djava.library.path=${natives_directory}/java`，会解析出真实子目录并只解压到那里（防目录穿越：`actualNativeFolder.startsWith(javaNativeFolder)`）。
5. **NativePatcher**（HMCL 侧，`HMCL/util/NativePatcher.java`）：按平台用内置 `assets/natives.json` 替换/删除 lwjgl natives；`needPatchMemoryUtil()` 检测 Java 25/26 + lwjgl 3.4.1 的 `MemoryUtil` 问题并挂 `lwjgl-unsafe-agent`。

### 3.5 变量表（`DefaultLauncher.getConfigurations()`）

| 变量 | 值 |
|---|---|
| `${auth_player_name}` `${auth_uuid}` `${auth_access_token}` `${auth_session}` `${user_type}` `${user_properties}` | 来自 `AuthInfo`；uuid 用 `UUIDs.toCompactString`（无连字符） |
| `${version_name}` `${version_type}` `${profile_name}` | 来自 `LaunchOptions` / manifest |
| `${game_directory}` | `instance.getRunDirectory()` 绝对路径 |
| `${assets_root}` `${game_assets}` | `instance.getActualAssetDirectory(assetId)`（虚拟资源会重建） |
| `${assets_index_name}` | `manifest.getAssetIndex().getId()` |
| `${classpath}` `${classpath_separator}` `${library_directory}` `${libraries_directory}` `${primary_jar}` `${primary_jar_name}` `${file_separator}` | HMCL 自建（后三个是 HMCL 方言，用于 `-DignoreList`） |
| `${natives_directory}` | natives 目录（可能含 `/tmp` 前缀） |
| `${resolution_width}` `${resolution_height}` | 窗口尺寸 |
| `${launcher_name}` `${launcher_version}` | HMCL 在子类 `HMCLGameLauncher.getConfigurations()` 注入（`Metadata.NAME/VERSION`） |
| `${language}` | `Locale.getDefault().toLanguageTag()` |

### 3.6 Log4j 配置

- 判定：`isUsingLog4j()` = 游戏版本 >= 1.7。
- 文件：写到 **`instanceRoot/log4j2.xml`**（`getLog4jConfigurationFile()`），每次启动覆盖。
- 模板四选一（`HMCLCore/src/main/resources/assets/game/`）：
  `log4j2-1.7.xml` / `log4j2-1.7-debug.xml`（<1.12）、`log4j2-1.12.xml` / `log4j2-1.12-debug.xml`（>=1.12）。
- 传参：`-Dlog4j.configurationFile=<abs>`，并强制 `-Dlog4j2.formatMsgNoLookups=true`（除非用户显式写 false）。

> 这套"**由启动器接管游戏日志格式**"的做法非常值得抄：只有把日志格式固定下来（含时间戳、级别、logger 名），启动器才能可靠地解析崩溃、判断"游戏窗口已打开"、做日志染色。

### 3.7 进程启动、监控与日志收集

**启动**（`DefaultLauncher.launch()`）：

```java
ProcessBuilder(rawCommandLine).directory(runDirectory)
if (listener == null) builder.inheritIO();       // 无监听时直接继承 IO
builder.environment().put("APPDATA", gameDir.getParent());   // Windows 沙盒
builder.environment().putAll(getEnvVars(nativeFolder));
process = builder.start();
ManagedProcess p = new ManagedProcess(process, rawCommandLine);
if (listener != null) startMonitors(p, nativeFolder, listener, encoding, daemon);
```

**环境变量**（`getEnvVars()`）除了渲染后端（`GALLIUM_DRIVER`、`VK_ICD_FILENAMES`、`__NV_PRIME_RENDER_OFFLOAD`、`MESA_LOADER_DRIVER_OVERRIDE=zink` …）外，还注入一套"实例元信息"给启动脚本/包装器使用：
`INST_NAME / INST_ID / INST_DIR / INST_MC_DIR / INST_JAVA`，以及各 `ModLoaderType.getEnvVarName()` 与 `INST_OPTIFINE=1`。

**监控**（`startMonitors()`）：

```
Thread stdout = StreamPump(process.getInputStream(), line -> { listener.onLog(line,false); process.addLine(line); })
Thread stderr = StreamPump(process.getErrorStream(), line -> { listener.onLog(line,true);  process.addLine(line); })
Thread exit   = ExitWaiter(process, [stdout, stderr], (code, type) -> { listener.onExit(...); runPostExitCommand(); })
```

`launch/ExitWaiter.java` 的**退出类型推断**（用日志内容判定，很实用）：

| 条件 | `ExitType` |
|---|---|
| `exit != 0` 且日志含 `Could not create the Java Virtual Machine.` / `Error occurred during initialization of VM` / `A fatal exception has occurred...` | `JVM_ERROR` |
| `exit != 0` 或日志含 `Crash report saved to` / `Unable to launch` / `An exception was thrown...` | `APPLICATION_ERROR`（Linux 下 `137` → `SIGKILL`） |
| 其他 | `NORMAL` |
| 被打断 | `INTERRUPTED` |

并同时发全局事件：`JVMLaunchFailedEvent` / `ProcessExitedAbnormallyEvent` / `ProcessStoppedEvent`（`event/EventBus.java`）。

**HMCL 侧日志消费**（`LauncherHelper.HMCLProcessListener`）：

- **访问令牌脱敏**：`log.replace(forbiddenAccessToken, "<access token>")`。
- **ANSI 转义清洗**：`StringUtils.parseEscapeSequence(log)`。
- **级别推断**：stderr 默认 ERROR（`[authlib-injector]` 除外），stdout 用 `Log4jLevel.guessLevel(log)`。
- **批量刷 UI**：`LinkedBlockingQueue` + 200ms 合批 + `Semaphore` 背压，避免每行一次 `Platform.runLater`。
- **判定"游戏窗口已打开"**：首次出现 `lwjgl version` / `lwjgl openal` 即认为启动成功（用于 `LauncherVisibility`：KEEP/HIDE/HIDE_AND_REOPEN/CLOSE 与倒计时关闭启动器）。
- **异常退出**：`markLaunchedAbnormally()` + 弹 `GameCrashWindow`（读最后 N 行 + `CrashReportAnalyzer` 解析 crash-report）。

**日志环形缓冲**：`CircularArrayList<Log>(Log.getLogLines()+1)`，固定容量，永不无界增长。

### 3.8 Rust 侧最小实现建议

```rust
pub struct LaunchPlan {            // == LaunchOptions + manifest
    java: PathBuf, jvm_args: Vec<Cow<'static, str>>, main_class: String,
    game_args: Vec<String>, classpath: Vec<PathBuf>, natives: PathBuf,
    env: BTreeMap<String, String>, cwd: PathBuf,
}
// 1) 组装（纯函数，可单测）
fn build(manifest: &ResolvedManifest, opts: &LaunchOptions, auth: &AuthInfo, platform: &Platform) -> Result<LaunchPlan>;
// 2) 变量替换（单遍扫描，未命中保留原文）
fn expand(arg: &str, vars: &HashMap<&str, String>) -> String;
// 3) 执行 + 监控
fn spawn(plan: &LaunchPlan) -> Result<Child>;    // stdout/stderr 两个 reader task + wait task
```

**必须保留的 4 个细节**：`-Duser.home` 沙盒、`-Dlog4j.configurationFile` 接管、natives 目录含实例 id、`ExitType` 用日志正则兜底。

---

## 4. 下载体系

### 4.1 三层抽象

```
DownloadProvider            （每"源集合"一个实例：官方 / BMCLAPI / 自定义）
  ├─ getVersionsAsync(type, gameVersion, refresh) → Task<ComponentRemoteVersionList<V>>
  ├─ getDownloadCandidates(urls)                   → DownloadCandidates（保序候选）
  ├─ getAssetObjectCandidates(assetObject)         → DownloadCandidates
  ├─ getCacheRepository()                          → DefaultCacheRepository
  └─ 各类 version-list 的 candidates（game / fabric / cleanroom / …）

ComponentRemoteVersion      （一个可安装的远端组件版本）
  ├─ componentType, gameVersion, selfVersion, releaseDate, urls, type(RELEASE/SNAPSHOT/…)
  └─ getInstallTask(depMgr, manifest, modsDir) → Task<GameInstancePatch>   ← 关键：安装=产 patch

ComponentRemoteVersionList  （不可变数组 + `getRemoteVersion(id)` 查找）
```

`DownloadProvider.getVersionsAsync` 内置**每组件一份的读写锁缓存**（`VersionListState` + `VarHandle` 数组 + `SoftReference`），并提供 `refresh` 参数强制刷新。

### 4.2 镜像适配：URL 前缀重写 + 候选排序

`HMCL/download/HMCLDownloadProvider.java` 是全部镜像逻辑所在（**没有为每个镜像写一个 Provider 子类**）：

```java
rules = [
  new MirrorRule("https://launchermeta.mojang.com", bmclRoot),
  new MirrorRule("https://piston-meta.mojang.com", bmclRoot),
  new MirrorRule("https://libraries.minecraft.net", bmclRoot + "/libraries"),
  new MirrorRule("https://maven.fabricmc.net", bmclRoot + "/maven"),
  new MirrorRule("https://maven.neoforged.net/releases/", bmclRoot + "/maven/"),
  new MirrorRule("https://api.modrinth.com", "https://mod.mcimirror.top/modrinth", /*fallback*/ true),
  ...
]
```

候选顺序由三件事决定：

1. **地区**：`RegionUtils.isIsChinaMainlandMachine()`（依据时区/语言等启发式）。
2. **用户设置**：`DownloadSource { DEFAULT, OFFICIAL, MIRROR }`（文件源与版本清单源分开设置，`setFileSource` / `setVersionListSource`）。
3. **规则是否 `fallback`**（fallback=true 的镜像**只能兜底**，不能优先——用于 modrinth/curseforge 这类"镜像可能缺文件"的场景）。

组合结果（中国大陆 / 其他地区）：

| 设置 | 中国大陆 | 其他 |
|---|---|---|
| DEFAULT | `[mirror, official]` | `[official]` |
| OFFICIAL | `[official, mirror]` | `[official]` |
| MIRROR | `[mirror, official]` | `[mirror, official]` |

另外两处特殊：
- **Forge/NeoForge 版本清单**：`FallbackTask`（一个 task 失败后**动态追加** fallback 依赖），官方与 BMCL 两份清单互为兜底（`HMCLDownloadProvider.fetchVersionsAsync`）。
- **OptiFine 只有 BMCL 源**（官方无 API）。

> **对 Rust**：镜像不要做成 trait 实现（`trait Mirror { fn rewrite(url) }` 会爆炸）。做成**数据驱动的 `Vec<RewriteRule>` + 排序策略**，一个 `DownloadProvider` 结构体即可；`DownloadCandidates` 天然支持"重试时自动换源"。

### 4.3 传输层：`task/FetchTask.java`（750 行，全仓库最值得读的下载代码）

| 能力 | 实现 |
|---|---|
| **多候选顺序尝试** | `for candidate in candidates: try download; catch → collect exception`，全失败时把前面的异常 `addSuppressed` 到最后一个抛出 |
| **重试** | `DEFAULT_RETRY = 5`（每候选可单独配置），只对可恢复错误重试；`attemptsLimit++` 用于 401/403 换源等特殊情况 |
| **重定向** | 手动跟随（`3xx` 且非 304/306），记录 `redirects` 链，**上限 20 次**（"Too much redirects"），拒绝非 HTTP 目标 |
| **HTTP 缓存** | `CacheRepository.getCachedRemoteFile(url, checkETag)`：`etag.json` 记录 `ETag/Last-Modified`，命中则直接 `useCachedResult(cache)` |
| **断点续传** | 先 HEAD/GET 得到 `HttpResumeContext`（要求 `accept-ranges: bytes`、`Content-Encoding: identity`、有强 `ETag` 或 `Last-Modified`、`Content-Length` 已知）；续传时带 `Range: bytes=<n>-` + `If-Range: <etag|last-modified>`；用 `Content-Range` 校验起始/总长；`hasPartialContent()` 决定是否保留临时文件跨重试续传；服务端回 `416` 时丢弃续传上下文、保留已下载部分并重试 |
| **压缩编码** | `ContentEncoding`（gzip/br/deflate）包装流；续传时禁用（identity 才可续） |
| **完整性** | 子类给 `IntegrityCheck(algorithm, checksum)`；下载到临时文件 → 校验 → 原子 move |
| **并发闸门** | `DOWNLOAD_EXECUTOR`：Java 21+ 用虚拟线程 + `Semaphore(DEFAULT_CONCURRENCY)`；否则固定线程池。`DEFAULT_CONCURRENCY = min(4*CPU, 64)`（`FetchTask:680`），可由设置动态调整（`setDownloadExecutorConcurrency`，支持增减 permit） |
| **限速/测速** | `updateDownloadSpeed` + 每秒 `SpeedEvent` |
| **BMCLAPI 加速** | 读响应头 `x-bmclapi-hash`，若为 SHA1 且缓存已存在 → 直接命中缓存（**连下载都省了**） |

### 4.4 缓存与校验

两种缓存，职责不同：

**（A）`util/CacheRepository.java` —— URL/内容双寻址**
```
<cacheDir>/etag.json                      # URL → {etag, lastModified, localFile}
<cacheDir>/<ALG>/<hash[0:2]>/<hash>       # 内容寻址
```
- `checkExistentFile(candidate, alg, hash)`：存在且摘要匹配才返回。
- `getCachedRemoteFile(url, checkETag)`：按 URL 的 etag 命中。
- 有 `ReadWriteLock` + `FileLock`（多进程安全）。

**（B）`download/DefaultCacheRepository.java` —— 库/资源的"二级缓存"**
```
<commonDir>/cache/libraries/index.json    # name → {hash, path}
<commonDir>/libraries/...                 # 实际库文件（可作为新实例的复制源）
```
- `tryCacheLibrary(lib, jar)`：**只有校验通过才缓存**（SHA1 或 jar 内 `checksums.sha1`）。
- `getLibrary(lib)`：先按 SHA1 找，再按 name 查 index（Forge 的库经常换 hash 但同名）。

**校验链**（`download/game/GameLibrariesTask.shouldDownloadLibrary()`）：
1. 文件不存在 → 下载
2. `!integrityCheck` → 直接跳过（快速启动）
3. `library.getDownload().validateChecksum(file, true)`（SHA1）
4. `library.checksums()` 非空 → `LibraryDownloadTask.checksumValid`（支持 jar 内 `checksums.sha1` 逐文件校验，**用于 Forge 的"重打包 jar"**）
5. `.jar` 结尾 → `ZIP_INTEGRITY_CHECK_HANDLER.checkIntegrity`（防截断/损坏）

### 4.5 文件级任务编排

所有下载都是"**父任务在 `execute()` 里生成子任务列表**"：

| 任务 | 粒度 | 备注 |
|---|---|---|
| `GameDownloadTask` → `CacheFileTask` | client jar 一份 | 只下载到缓存，**commit 时才复制进 `versions/<id>/<id>.jar`**（`DefaultGameBuilder.buildAsync` 的 `draft.putPrimaryJar`） |
| `GameAssetIndexDownloadTask` | 索引一份 | `verifyHashCode = url.contains(sha1)` 才校验（Mojang 会改索引文件） |
| `GameAssetDownloadTask` | **每个 asset 一个 `FileDownloadTask`** | 先确保索引，再遍历 objects；命中缓存则 `tryCacheFile` |
| `GameLibrariesTask` | **每个 library 一个 `LibraryDownloadTask`** | 特殊处理：FML 老库（`getFMLLibs`）、内嵌 `mmc-bootstrap`/`transformer-discovery-service`、OptiFine 1.20.4 的 `mods.toml` 删除 |
| `LibraryDownloadTask` | 库 | `preExecute` 查缓存 → `FileDownloadTask` → `postExecute` 回写缓存；`net.minecraftforge:forge` 强制 classifier=`universal` |
| `CacheFileTask` | 内容寻址 | 直接以 SHA1 为 key |

### 4.6 Java 运行时下载与版本匹配

**两条供应链**（`download/java/`）：

| 来源 | 协议 | 类 |
|---|---|---|
| Mojang 官方 | `piston-meta/.../java-runtime/all.json` → 每平台每组件一个 `manifest` → 逐文件 `raw` / `lzma` | `download/java/mojang/MojangJavaDownloadTask.java` |
| Disco API（Adoptium/Zulu/Liberica/GraalVM/Semeru/Corretto） | `api.foojay.io` 风格 JSON | `download/java/disco/DiscoJavaDistribution.java`、`DiscoJavaRemoteVersion.java` |

Mojang 侧细节：
- 选版本：`JavaInfo.parseVersion(download.version().name()) >= javaVersion.majorVersion()`。
- 逐文件：`RemoteFile`（`raw` + 可选 `lzma`，优先 lzma 省流量）、`RemoteDirectory`、`RemoteLink`（符号链接）。
- **先写 `tempDir` 再整体 move 到 `target`**（`postExecute`），跨文件系统则 copy+delete —— 保证"要么全有要么全无"。
- 可执行位：`file.isExecutable()` → `FileUtils.setExecutable`。

Disco 侧：`JavaDistribution` 枚举声明每个发行版支持的 `JavaPackageType { JRE, JDK, JREFX, JDKFX }` 与 `(OS, Arch) → 支持矩阵`（如 Temurin 支持 `LINUX + {X86_64,X86,ARM64,ARM32,RISCV64,PPC64,PPC64LE,S390X,SPARCV9}`）。

**版本匹配规则**（`game/JavaVersionConstraint.java`，一张规则表，非常重要）：

| 规则 | 强制? | 适用 | 判定 |
|---|---|---|---|
| `VANILLA` | ✅ | 版本 JSON 无 `javaVersion` | 按 `GameJavaVersion.getMinimumJavaVersion()`：>=26.1→25、>=1.20.5→21、>=1.18→17、>=1.17→16、>=1.13→8 |
| `GAME_JSON` | ✅ | 1.7.10+ 且有 `javaVersion` | `java >= 声明值` |
| `LAUNCH_WRAPPER` | ✅ | <=1.12 且 mainClass 是 launchwrapper | `java <= 8`（URLClassLoader 假设） |
| `VANILLA_LINUX_JAVA_8` | ✅ | Linux + x86_64 + <=1.12 | 必须 64 位 Java 8 |
| `VANILLA_JAVA_8_51` | ❌ | 1.13+ | >= 1.8.0_51 |
| `MODDED_JAVA_7/8/16/17/21` | ❌ | 对应 Forge 版本段 | 建议值 |
| `CLEANROOM` | ✅ | 1.12.2 + cleanroom | 按 cleanroom 版本 |
| `MODLAUNCHER_8` | ❌ | 1.16.3–1.17.1 Forge | 避开 JDK-8273826 的版本区间 |
| `VANILLA_X86` | ❌ | ARM64 | 老版本需要 x86 Java |

配套还有 `GameJavaVersion.getSupportedVersions(platform)`：**平台 → 可自动下载的 Java 大版本集合**（如 Windows x86 只有 8/16/17，Linux x86 只有 8，ARM64 Windows/macOS 只有 17/21/25）。

安装目录（`HMCL/java/HMCLJavaRepository.java`）：
```
<javaRoot>/<platform>/mojang-<component>/...      # 如 linux-x86_64/mojang-java-runtime-delta/
<javaRoot>/<platform>/mojang-<component>.json     # JavaManifest（os/arch/version/vendor + files 校验信息）
```
`JavaManifest` 里还带 `files` 的本地校验信息（`JavaLocalFiles.Local`），用于"运行时是否被破坏"的快速检查；`JavaManager.Searcher` 用 `release` 文件的 SHA1 + `lib/modules` 属性做**目录扫描缓存 key**。

### 4.7 Rust 设计建议

- `Downloader` 用 `reqwest` + `tokio::sync::Semaphore`；候选列表用 `SmallVec<[Url; 2]>`，**不要**为镜像设计 trait。
- 缓存一定要**两级**：URL 级（ETag）用于 JSON/清单，内容级（SHA1）用于所有 jar/asset。目录结构照抄 `cache/<alg>/<xx>/<hash>`。
- 断点续传只在"大文件 + 强校验"场景开（client jar、java runtime）；小文件重下更划算。
- 并发默认 `min(4*cores, 64)` 且可运行时调整——HMCL 的 `Semaphore` 增减 permit 实现（`FetchTask.setDownloadExecutorConcurrency`）可以直接抄。

---

## 5. 任务模型

### 5.1 `Task<T>`：一个"双向 DAG 节点"

`HMCLCore/src/main/java/org/jackhuang/hmcl/task/Task.java`（1296 行）核心约定：

| 概念 | 字段/方法 | 说明 |
|---|---|---|
| 前置 | `getDependents()` | 在**本任务之前**执行的子任务 |
| 后置 | `getDependencies()` | 在**本任务之后**执行的子任务（失败则不执行） |
| 生命周期 | `doPreExecute()` → `preExecute()` → `execute()` → `postExecute()` → `doPostExecute()` | `do*` 返回 bool 表示"是否需要在执行前/后插一段" |
| 结果 | `setResult(T)` / `getResult()` / `storeTo(Consumer)` | 结果**只能在成功后**读取 |
| 进度 | `progressProperty()`（JavaFX DoubleProperty）、`updateProgress(count,total)` | 1 秒节流（`lastUpdateProgressTime`），合并到 FX 线程 |
| 阶段 | `stage` / `inheritedStage` | 子任务默认继承父 stage；用于 UI 分组与提示 |
| 重要性 | `TaskSignificance { MAJOR, MODERATE, MINOR }` | 决定是否写日志、是否在 UI 显示 |
| 取消 | `isCancelled()`（Thread 中断 + 外部 `BooleanSupplier`） | 协作式 |
| 组合语义 | `isRelyingOnDependents()` / `isRelyingOnDependencies()` | 可关掉"必须等"的默认行为（`FallbackTask` 就靠这个实现"先试 A，失败再挂 B"） |
| 异常 | `setException` / `getException` | 取消时 exception 为 null |

**执行模型**（`run()`）：`for d in dependents: doSubTask(d)` → `execute()` → `for d in dependencies: doSubTask(d)`；`doSubTask` 会把子任务进度 `bind` 到父进度（JavaFX 属性绑定实现"聚合进度"）。

### 5.2 组合子（这是 Task 最好用的部分）

| 组合子 | 语义 |
|---|---|
| `thenComposeAsync(fn)` | 成功后串接另一个 task（fn 可返回 task） |
| `withRunAsync(action)` | 成功后执行一段动作（不改变结果） |
| `withComposeAsync(fn)` | 失败也执行（类 finally，结果透传） |
| `whenComplete(cb)` | 完成/失败都回调，可拿到异常 |
| `Task.allOf(tasks)` | **并行**等待全部（依赖集合） |
| `Task.combine(t1, t2)` | 并行 + 结果配对 |
| `Task.runSequentially(t1..tn)` | 顺序链 |
| `Task.composeAsync(fn)` | 惰性构造（用于"运行时才知道要跑什么"） |
| `withCounter(stage)` | 登记"本任务算一个计数项"，配合 `getProperties().put("total", n)` 做 `x/n` 进度 |
| `withStagesHint(...)` | 声明阶段树的父子关系（`Task.StagesHint("launch.state.dependencies", List.of("hmcl.install.assets", ...))`） |
| `withFakeProgress(name, done, k)` | 对"无法量化"的阶段（等游戏窗口出现）用假进度 |
| `withStage("...")` | 打阶段标签 |

> `withCounter` + `properties["total"]` + `StagesHint` 三者组合，构成了 HMCL 的**进度显示协议**——相当轻量，不需要中心化的进度树。

### 5.3 执行器

`task/AsyncTaskExecutor.java`（354 行）：
- 用 `CompletableFuture` 递归展开：`executeTask(parent, task)` → 并行展开 `getDependents()` → `execute()` → 展开 `getDependencies()`。
- `TaskListener` 回调点：`onStart` / `onReady` / `onRunning` / `onFinished` / `onFailed` / `onStop(success, executor)` / `onPropertiesUpdate`。
- `cancel()` 只是置 `cancelled = true`；任务内部靠 `isCancelled()`（含 `Thread.interrupted()`）协作退出，**不会强杀线程**。
- 异常处理：`resolveException` 解包 `CompletionException`；`RuntimeException`（非取消/非 JSON/非拒绝执行）交给 `uncaughtExceptionHandler`；`OutOfMemoryError` 特殊处理。
- 执行线程：`Schedulers.defaultScheduler()` = `ForkJoinPool.commonPool()`；IO = 虚拟线程（21+）或 cached pool；UI = `Platform::runLater`。

### 5.4 缺陷（Rust 不要复制）

1. **图在执行期才确定**：`execute()` 里向 `dependencies` 里 `add`，导致"任务图"不可静态分析、不可序列化、不可断点续跑。
2. **取消是协作式的**：一个阻塞在 `InputStream.read` 的下载任务不会立刻停（只能等 socket 超时）。
3. **进度靠 JavaFX 属性绑定**：core 被 UI 线程模型污染；`updateProgressImmediately` 还必须 `Platform.runLater`。
4. **异常信息被层层包装**：`DownloadException` → `LibraryDownloadException` → UI 里再 switch 十几种类型拼文案（`LauncherHelper` 的 `onStop` 有 80 行 `instanceof` 分支）——类型化错误码会好得多。
5. **没有重试/回滚语义**：任务失败后已写入的文件保持原样（HMCL 靠"下次启动再补全"兜底）。

### 5.5 Rust 建议：`Stage` 树 + `CancellationToken`

```rust
pub trait Step: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, cx: &mut Ctx) -> Result<()>;   // cx: progress, cancel, fs, http
}
// 组合：Seq / Par(bounded) / Try(seq of fallbacks) / ForEach(items)
pub enum Plan { Step(Box<dyn Step>), Seq(Vec<Plan>), Par { limit: usize, items: Vec<Plan> } }
```
- 用 `tokio_util::sync::CancellationToken` 替代 `isCancelled()` 轮询；
- 进度用 `watch::Sender<Progress>`，由 UI 决定如何渲染（core 零 GUI 依赖）；
- 每个 `Step` 自报 `weight`，父节点按权重聚合——比 HMCL 的 `withCounter` 更通用；
- 错误用 `thiserror` 枚举（`Download{url,status}` / `Checksum{expected,actual}` / `MissingJava{required}` …），UI 只做"错误码 → i18n key"映射。

---

## 6. 模组加载器与整合包

### 6.1 组件识别：`GameComponentType`（枚举即注册表）

`game/GameComponentType.java`（277 行）为每个组件声明：
- `patchId`（写进 `patches[].id`，如 `"forge"`、`"fabric"`、`"optifine"`）
- 可选的 `ModLoaderType`（给 UI / 环境变量用）
- `matchLibrary(library, allLibraries)`：**从库坐标反推组件**（例如 `net.fabricmc:fabric-loader` → FABRIC；`net.neoforged.fancymodloader:core|loader` → NEO_FORGE；`net.minecraftforge:forge|fmlloader|minecraftforge` → FORGE，但要先排除 NeoForge）
- `getComponentVersion(...)`：有的要从库版本号里正则抽取（`^([0-9.]+)-(?<forge>[0-9.]+)...`），有的要扫 patches。

`game/GameComponentAnalyzer.java` 的识别顺序：**先看 patches（权威）→ 再看库坐标（启发式）→ 最后靠 `GameVersionNumber` 补 GAME**。这套"枚举 + 谓词"的注册表非常适合 Rust 的 `enum ComponentKind { fn matches(&Library) -> Option<Version> }`。

### 6.2 安装器矩阵

| 加载器 | 类 | 机制 | 是否跑外部进程 |
|---|---|---|---|
| Fabric | `download/fabric/FabricInstallTask.java` | 拉 `meta.fabricmc.net` 的 loader JSON（`launcherMeta`），直接构造 patch：`mainClass` + `libraries(common+server)` + intermediary/loader 两个 maven 库 | ❌（纯 JSON） |
| Quilt / LegacyFabric | `QuiltInstallTask` / `LegacyFabricInstallTask` | 同上（Fabric 系同构） | ❌ |
| Forge（新，1.13+） | `ForgeNewInstallTask`（460 行） | `install_profile.json` 的 `spec` + `processors`，见 §6.3 | ✅ 每个 processor 一次 JVM |
| Forge（旧，<=1.12） | `ForgeOldInstallTask` | `install_profile.json` 的 `install` + `versionInfo`，直接把 `versionInfo` 当 patch，安装 `install.filePath` 到 `libraries/` | ❌ |
| NeoForge | `NeoForgeInstallTask` | 复用 `ForgeNewInstallTask`（同一套 processor 协议） | ✅ |
| Cleanroom | `CleanroomInstallTask` | 同 Forge 新格式 | ✅ |
| OptiFine | `OptiFineInstallTask` | 跑 `optifine.Patcher` 生成 patched jar，见 §6.4 | ✅ 1 次 |
| LiteLoader | `download/liteloader/LiteLoaderInstallTask.java` | 纯 patch：装 `com.mumfrey:liteloader` 库 + `--tweakClass com.mumfrey.liteloader.launch.LiteLoaderTweaker`，priority=60000；并用 `.withLogging(emptyMap())` **关掉官方 log4j 配置**（模组日志格式不合规会让 XML 解析崩） | ❌ |
| Fabric API / Quilt API | `FabricAPIInstallTask` | 从 Modrinth 取文件，装进 `mods/` | ❌ |

**统一出口**：全都返回 `Task<GameInstancePatch>`，由 `DefaultDependencyManager.installUnpublishedComponentAsync()` 统一做
`manifest.removeComponent(type) → installTask → manifest.addPatch(patch).reconstructByPatches()`。
这就是"安装 = 产 patch"的好处：**新增一种加载器不需要改版本模型、不需要改启动器**。

### 6.3 Forge installer processors（唯一真正复杂的安装器）

`download/forge/ForgeNewInstallTask.java` 完整流程：

**① 读 installer**（`preExecute`）
```java
try (FileSystem fs = createReadOnlyZipFileSystem(installer)) {
    profile = json(fs/"install_profile.json", ForgeNewInstallProfile.class);
    forgeVersion = json(fs/profile.getJson(), GameInstanceManifest.class);   // 要装进去的版本
    for (lib in profile.getLibraries())                    // installer 内嵌 maven/
        copy(fs/"maven"/lib.getPath(), layout.getLibraryFile(id, lib));
    if (profile.getPath() != null) copy main jar;
}
dependents.add(new GameLibrariesTask(depMgr, manifest, true, profile.getLibraries()));
```

**② 变量表**（`execute`）
```
vars = profile.data 的 literal 解析（{KEY} 引用、'literal'、[group:name:version] → 库绝对路径、其余按普通字符串）
     + 提取 installer 内嵌文件到临时目录（用于 SIDE 等）
vars += { SIDE: "client", MINECRAFT_JAR: <临时目录里的 client.jar>,
          MINECRAFT_VERSION: profile.minecraft, ROOT: <baseDir>,
          INSTALLER: <installer 绝对路径>, LIBRARY_DIR: <libraries 绝对路径> }
```
> **关键安全设计**：`minecraft.jar` 先复制到 `Files.createTempDirectory("forge_installer")` 再传给 processor（注释："External processors must not receive the shared cache path"）——避免 installer 污染/移动共享缓存。

**③ 顺序执行 processors**（`Task.runSequentially`）
每个 `ProcessorTask`：
1. 解析 `outputs`（`{path: sha1}`），若文件都已存在且 SHA1 匹配 → **整体跳过**（增量安装）。
2. 找 `processor.jar`（`layout.getArtifactFile`），读 `META-INF/MANIFEST.MF` 的 `Main-Class`。
3. 组命令：`java -cp <processor.classpath...>:<jar> <mainClass> <args...>`（args 逐个 `parseLiteral`）。
4. `SystemUtils.callExternalProcess`，非 0 退出 → 异常。
5. 校验每个 output 的 SHA1；**zlib 兼容性例外**：若本机用 zlib-ng 等非标准压缩库导致 jar 字节不同，退化为"ZIP 结构合法"检查并通过（非常有现实意义的兼容代码）。
6. 进度：`processorDoneCount.incrementAndGet() / processors.size()`。

**④ 特殊 processor：`DOWNLOAD_MOJMAPS`**
`patchDownloadMojangMappingsTask`：解析 args 里的 `task/side/version/output`，若不是 `DOWNLOAD_MOJMAPS`+`client` 返回 null（走正常 processor）；否则**不跑 JVM**，改为下载 `client_mappings` 到 `output`。这是"用任务替换外部进程"的典型手法。

**⑤ 收尾**：`dependencies.add(processorsTask.thenComposeAsync(depMgr.checkComponentCompletionAsync(forgeVersion, true)))`，`setResult(GameInstancePatch.fromManifest(forgeVersion, "forge", selfVersion, PRIORITY_LOADER))`，`postExecute` 删临时目录。

**新旧格式判别**（`ForgeInstallTask.detectForgeInstallerType`）：
`install_profile.json` 有 `spec` → 新（processor）；有 `install` + `versionInfo` → 旧；否则报错。

### 6.4 OptiFine：唯一"改 jar"的安装器

`download/optifine/OptiFineInstallTask.java`：

1. 校验前置条件：manifest 的 mainClass 必须在 `FORGE_OPTIFINE_MAIN` 集合里（否则 `UNSUPPORTED_LAUNCH_WRAPPER`）。
2. 把 installer 复制成 `libraries/optifine/OptiFine/<mc>_<of>/OptiFine-...-installer.jar`，并**删掉其中的 `META-INF/mods.toml`**（否则 Forge 1.20.4 会崩）。
3. 若 installer 内有 `optifine/Patcher.class`：
   `java -cp installer.jar optifine.Patcher <minecraft.jar> <installer.jar> <out.jar>`
   否则直接 copy installer 当 patched jar。
4. 处理 launchwrapper：`launchwrapper-2.0.jar` / `launchwrapper-of-<ver>.jar` → 装成库；都没有则补 `net.minecraft:launchwrapper:1.12`。
5. 检查 `buildof.txt` 与 Forge 1.17 的兼容性下限（`20210924-190833`）。
6. 产出 patch：`mainClass = LaunchWrapper`、`arguments.game += ["--tweakClass", "optifine.OptiFineTweaker"]`、`priority = 10000`。
7. 本地 installer 识别（`install()` 静态方法）：**扫 `Config.class` 的常量池**，按 `MC_VERSION` / `OF_EDITION` / `OF_RELEASE` 三个 UTF8 常量定位版本——不需要版本 JSON，非常巧。

### 6.5 整合包

**抽象**（`modpack/`）：

```java
interface ModpackProvider {
    String getName();                                                        // 存进 modpack.json 的 type
    @Nullable Task<?> createCompletionTask(depMgr, instance);                 // 补全
    Task<?> createUpdateTask(depMgr, instance, zip, modpack, excludedFiles);  // 更新
    Modpack readManifest(ZipArchiveReader zip, Path file);                    // 识别+解析
    default void injectLaunchOptions(String cfgJson, LaunchOptions.Builder);   // 注入启动参数
    default ModpackManifest loadFiles(DownloadProvider, ModpackManifest);     // 补远端元数据
}
```

**6 个实现**（`HMCL/game/ModpackHelper.java` 注册表）：

| Provider | 识别文件 | 特点 |
|---|---|---|
| `McbbsModpackProvider`（`modpack/mcbbs`） | `mcbbs.packmeta` | 国内 MCBBS 格式，带 `launch` 高级设置（内存/Java/启动参数），需要 `injectLaunchOptions` |
| `CurseModpackProvider`（`modpack/curse`） | `manifest.json` | `files[{projectID,fileID,required,hashes}]`；URL 可由 `edge.forgecdn.net/files/<id/1000>/<id%1000>/<name>` 推导 |
| `ModrinthModpackProvider`（`modpack/modrinth`） | `modrinth.index.json` | `.mrpack`，文件直接给 `downloads[]` + `hashes` |
| `HMCLModpackProvider`（`HMCL/game`） | `modpack.json` | HMCL 自有格式 |
| `MultiMCModpackProvider`（`modpack/multimc`） | `mmc-pack.json` + `instance.cfg` | 7 阶段转换（注释里写得很清楚），要解析 JSON Patch 与 `patches/` |
| `ServerModpackProvider`（`modpack/server`） | `server-manifest.json` | 服务端整合包 |

**识别顺序**（`ModpackHelper.readModpackManifest`）：**按固定顺序逐个 try**（Mcbbs → Curse → Modrinth → HMCL → MultiMC → Server），全失败再尝试"手工整合包"（找 `minecraft` 目录）。注释明确警告"顺序是必要的，不要改成遍历"——因为多个格式都叫 `manifest.json`，靠 schema 校验互斥。

**安装/更新的关键策略**（`modpack/ModpackInstallTask.java`）：

```java
for each override entry:
   if (旧配置里没有该文件)            → 覆盖（可能是用户新增）
   else if (目标文件不存在)            → 跳过（用户删了就别塞回来）
   else if (SHA1(目标) == 旧记录的 hash) → 覆盖（用户没改过）
   else                               → 跳过（用户改过，保留）
for each 旧配置里有、新包没有的文件     → 删除
```
即"**以用户修改为准的三方合并**"，`ModpackConfiguration.overrides` 用 `{path, hash}` 记录安装时的状态。这套策略可以直接抄。

**补全**（`CurseCompletionTask` / `ModrinthCompletionTask`）：启动前比对 `manifest` 与磁盘，缺啥下啥；可选文件用 `excluded.json` 记录被用户排除的 key（`ModpackFile.key()` 如 `"curseforge:<pid>:<fid>"`）。

### 6.6 Rust 建议

- 加载器安装器统一签名 `async fn install(&self, ctx) -> Result<Patch>`；`Patch` 用 `serde_json::Value` + 强类型视图双轨（保留未知字段，见 §8.3-③）。
- Forge processor 执行器单独抽 crate（`nl-install-forge`），因为它是唯一需要"拼 JVM 命令行 + 校验产物哈希"的地方；务必保留**临时目录隔离 minecraft.jar**和**跳过已满足的 outputs** 两个优化。
- 整合包 provider 用 `trait + 静态注册表`，识别顺序显式写死。

---

## 7. 账户

### 7.1 分层

```
Account (abstract)                      auth/Account.java
 ├─ 身份：AccountID（稳定条目 id，不等于 profileID）
 ├─ getProfileName()/getProfileID()
 ├─ logIn() / playOffline() / logInWhenCredentialsExpired()
 ├─ writeMetadata(JsonObject)          → 公开数据（accounts.json）
 ├─ writePrivateData(JsonObject)       → 私密数据（account-private-data，单独文件）
 └─ getTextures()                       → 皮肤

OAuthAccount → MicrosoftAccount                （微软）
YggdrasilAccount → AuthlibInjectorAccount      （外置登录）
OfflineAccount                                  （离线）
ClassicAccount                                  （旧版 Mojang）
```

**存储分离**是这里最值得学的设计：
- `accounts.json`（`SettingsManager.GAME_ACCOUNTS_LOCATION`）：只有 `accountID / profileID / profileName / serverBaseURL` 等**可公开**字段。
- `user-accounts.json` / `account-private-data/*`：`accessToken / refreshToken / notAfter`（`MicrosoftSession.writePrivateData`）。
- 好处：分享配置、云同步、日志导出时不会泄漏 token；`Account.writeMetadata` 的 javadoc 明确写"must not contain credentials"。

`AuthInfo`（`auth/AuthInfo.java`）是"启动期凭据"：
```java
AuthInfo(username, uuid, accessToken, userType, userProperties)
// userType: "msa" | "mojang" | "legacy"
Arguments getLaunchArguments(LaunchOptions)   // 默认 null；authlib-injector 在这里注入 -javaagent
```

### 7.2 微软登录（`auth/OAuth.java` + `auth/microsoft/MicrosoftService.java`）

**两条 OAuth 流程**：

**(A) 授权码 + PKCE（默认，体验最好）**
1. 本地起 HTTP 服务（`HMCL/game/OAuthServer.java`，NanoHTTPD，`localhost:<随机端口>`，回调路径 `/auth-response`）。
2. 生成 `state`（32 字节 base64url）与 `code_verifier`（64 字节 base64url），`code_challenge = base64url(sha256(verifier))`。
3. 打开浏览器：
   `https://login.live.com/oauth20_authorize.srf?client_id=..&response_type=code&redirect_uri=http://localhost:<port>/auth-response&scope=XboxLive.signin offline_access&prompt=select_account&code_challenge=..&state=..&code_challenge_method=S256`
4. 回调校验 `state`（不匹配 → 失败；缺失 → 失败），取 `code`（以及可选 `id_token`）。
5. `POST https://login.live.com/oauth20_token.srf`（`grant_type=authorization_code`, `code_verifier`, `redirect_uri`, `scope`）→ `{access_token, refresh_token}`。

**(B) 设备码（无浏览器/无本地端口时）**
1. `POST https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode` → `{user_code, device_code, verification_uri, expires_in, interval}`。
2. 展示 `user_code` 并打开 `verification_uri`。
3. 轮询 `.../oauth2/v2.0/token`（`grant_type=urn:ietf:params:oauth:grant-type:device_code`）：
   - `authorization_pending` → 继续
   - `slow_down` → `interval += 5000ms`
   - `expired_token` → 失败
   - 超过 `min(expires_in, 900s)` → 失败

**换取 Minecraft 令牌（两条流程共用 `authenticateViaLiveAccessToken`）**：
```
XBL:  POST https://user.auth.xboxlive.com/user/authenticate
      {Properties:{AuthMethod:"RPS", SiteName:"user.auth.xboxlive.com", RpsTicket:"d="+liveToken},
       RelyingParty:"http://auth.xboxlive.com", TokenType:"JWT"}
   → token + displayClaims.xui[0].uhs
XSTS: POST https://xsts.auth.xboxlive.com/xsts/authorize
      {Properties:{SandboxId:"RETAIL", UserTokens:[xblToken]}, RelyingParty:"rp://api.minecraftservices.com/", TokenType:"JWT"}
   → token（401 时按 errorCode 抛 XboxAuthorizationException）
MC:   POST https://api.minecraftservices.com/authentication/login_with_xbox
      {identityToken:"XBL3.0 x=<uhs>;<xstsToken>"} → {username, access_token, token_type, expires_in}
校验: GET https://api.minecraftservices.com/entitlements/mcstore   （必须 200，否则"没有游戏所有权"）
资料: GET https://api.minecraftservices.com/minecraft/profile      （拿 uuid/name，可能为 null → 无角色）
```
`notAfter = now + expires_in*1000`。

**缓存与刷新**（`MicrosoftAccount.logIn()`）：
```java
if (!authenticated || !hasProfileName || now > session.notAfter) {
    if (hasProfileName && service.validate(notAfter, tokenType, accessToken)) authenticated = true;  // 先试旧 token
    else {
        acquired = service.refresh(session);                 // refresh_token 换新
        if (acquired.profile().id() != session.profile().id()) throw "Selected profile changed";
        session = acquired; authenticated = true;
    }
}
```
- 凭据失效（`invalid_grant` + `AADSTS70000`）→ `CredentialExpiredException` → UI 走 `logInWhenCredentialsExpired()`（重新设备码登录，且校验 profileID 未变）。
- `service.validate()` = 先看本地 `notAfter` 是否过期，再实际 `GET /minecraft/profile` 探活（`ResponseCodeException` → 视为失效，`IOException` → 视为网络故障抛出，**不**降级为"重新登录"）。
- 每个账号还有一个 `ObservableOptionalCache<UUID, CompleteGameProfile>` 缓存皮肤属性（`profileRepository`），并用 binding 触发 `invalidate()`。

### 7.3 离线账户与 authlib-injector

**离线**（`auth/offline/OfflineAccount.java`）：
- `profileID` 可由用户名派生或用户指定（存 metadata）。
- `logInWithoutSkin()` 返回 `AuthInfo(name, uuid, UUIDs.toCompactString(randomUUID()), USER_TYPE_MSA, "{}")`。
  —— 注释说明：**故意用 `msa` 而不是 `mojang`**，否则部分服务器会报 invalid session。
- 若配置了皮肤（`Skin.Type != DEFAULT`），`logIn()` 会在后台准备 authlib-injector artifact，然后返回带 `-javaagent` 的 `AuthInfo`。

**authlib-injector**（`auth/authlibinjector/AuthlibInjectorAccount.java`）：
```java
new Arguments().addJVMArguments(
    "-javaagent:" + artifact.location() + "=" + server.getUrl(),
    "-Dauthlibinjector.side=client",
    "-Dauthlibinjector.yggdrasil.prefetched=" + base64(metadataJson))
```
- `artifact` 由 `AuthlibInjectorArtifactProvider` 提供：
  - 默认 `AuthlibInjectorExtractor`（`HMCL/setting/Accounts.java:createAuthlibInjectorArtifactProvider()`）——从 jar 内 `/assets/authlib-injector-<ver>.jar`（构建期 `embedResources(libs.authlib.injector)` 打入）解压到 `<user home>/dependencies/universal/`；
  - 可用 `-Dhmcl.authlibinjector.location=` 指定外部 jar（`SimpleAuthlibInjectorArtifactProvider`）。
- `prefetched` 是服务端 `/` 的 metadata（含 `signaturePublickey`、`skinDomains`），**预取并内联**可以避免游戏启动时的额外请求（也避免 TLS 问题）。
- 登录 + 拉 metadata + 拉 artifact 三者**并行**（两个 `CompletableFuture`），减少等待。

### 7.4 皮肤

| 场景 | 机制 |
|---|---|
| 正版/外置登录 | `auth/yggdrasil/YggdrasilService.getTextures(profile)` 解析 base64 的 `textures` 属性（`SKIN`/`CAPE`/`ELYTRA` + `model: slim/wide`）；HMCL 用 `TexturesLoader` 按 URL 的 hash 落盘到 `<HMCL_USER_HOME>/skins/<xx>/<hash>`，做内存/磁盘两级缓存，再合成 2D 头像（`Canvas` 裁剪） |
| 离线自定义皮肤 | `auth/offline/YggdrasilServer.java`：**本地起一个迷你 Yggdrasil 服务**（`/`, `/sessionserver/session/minecraft/profile/<uuid>`, `/textures/<hash>`），用内置 RSA 私钥对 `textures` 属性签名，`skinDomains = [127.0.0.1, localhost]`；游戏通过 authlib-injector 指向它，从而显示本地皮肤 |
| 皮肤上传 | `MicrosoftService.uploadSkin(accessToken, isSlim, file)` → `PUT /minecraft/profile/skins` |

---

## 8. 从"轻量"角度评估 HMCL

### 8.1 体量与启动成本

| 指标 | 数值 | 说明 |
|---|---|---|
| 源码总量 | 约 **165k 行** Java（core 65.4k + app 98.7k + boot 0.45k） | `find ... | xargs wc -l` |
| 类文件数 | 871（core 422 + app 443 + boot 6） | 其中 `ui/` 243 个 |
| 发布物 | 单个 shadowJar（约 10–20 MB 级）+ 原生 exe 壳 | `HMCL/build.gradle.kts` |
| 运行前置 | **JRE 17+，且最好带 JavaFX**；不带则现场下载补丁 | `HMCLBoot/Main.java:MINIMUM_JAVA_VERSION=17`、`EntryPoint.checkJavaFX()` |
| JVM 启动参数 | 一长串 `--add-opens`（20 条，写进 manifest 的 `Add-Opens`） | `HMCL/build.gradle.kts:addOpens` |
| 冷启动路径 | exe → 找 JRE → JVM 启动 → `Main`（Java 8 类）→ `EntryPoint`（可能下载 JavaFX）→ JavaFX 初始化 → 设置加载 → 主窗口 | 5 段跳转 |
| 内存 | `-Xmx1g`（`gradle run` 的默认 `HMCL_JAVA_OPTS`）；运行时占用为经验估计，未实测 | — |
| 测试 | 71 个 `*Test.java`，集中在 core（版本号、JSON、压缩、i18n） | `HMCLCore/src/test` |

**结论**：HMCL 的"重"不在业务逻辑，而在**运行时底座**：
1. 必须携带/下载一个 JRE（100MB+）与 JavaFX（另 50MB+）；
2. JavaFX 的线程模型（`Platform.runLater`）渗透到了 core（属性绑定、进度）；
3. 为兼容极老的 Forge/OptiFine，内置了大量写死的版本区间与 jar 补丁（`GameComponentAnalyzer.FORGE_OPTIFINE_BROKEN_RANGE`、`repairLaunchWrapper` 等）。

Rust 单二进制 + 原生 GUI 可以把 1、2 直接消掉；第 3 点是**领域知识**，与语言无关，必须照抄（否则用户体验会退化）。

### 8.2 模块化的真实收益

**收益**：
- `HMCLCore` 确实可以脱离 UI 使用：没有一处 import `HMCL`，所有 UI 交互都通过返回值/回调/Task 暴露。理论上可以写 CLI 或服务端复用（HMCL 自己也用它跑"仅安装"流程）。
- 组件化彻底：新增加载器 = 新增 `ComponentRemoteVersion` + `InstallTask`，不改版本模型、不改启动器。
- `GameRepositoryLayout` 让"官方布局 / MultiMC 布局"可插拔（`GameRepositoryLayout` 是接口）。

**不足**：
- core 依赖 JavaFX 的 observable/属性（35 个文件），无法在无 GUI 环境（headless server）直接跑。
- `download` 包同时承担"远端元数据"与"安装编排"两种职责，且与具体加载器强耦合（`download/forge`、`download/fabric` 都在 download 包下），包内聚性一般。
- `HMCL` 模块把"应用编排"（`LauncherHelper`）、"设置"（`setting/` 40+ 文件）、"UI"（243 文件）混在一起，`LauncherHelper` 里既有业务流程又有 80 行错误文案 switch——这是"重 UI 框架"的必然结果。

### 8.3 对 Rust 最有价值的 5 个设计

#### ① **Patch 化的版本合成（安装 = 产出 patch，启动 = 按 priority 合成）**
- 证据：`GameInstancePatch` + `GameInstanceManifest.Builder.merge()` + `DefaultGameRepositorySnapshot.resolve()`。
- 价值：把"安装"从"改写全局 JSON"变成"追加一条不可变记录"，天然支持：多加载器叠加、组件卸载、增量更新、幂等重装、失败回滚（丢弃 patch 即可）。
- Rust 落地：
  ```rust
  struct Patch { id: ComponentKind, version: String, priority: i32, libs: Vec<Library>,
                 main_class: Option<String>, jvm_args: Vec<Arg>, game_args: Vec<Arg>, /* ... */ }
  fn compose(base: &Manifest, patches: &[Patch]) -> Resolved;  // 稳定排序 + 后写覆盖
  ```
  落盘时**只写 patches**，`Resolved` 用 `#[serde(skip)] OnceCell` 缓存。

#### ② **候选列表 + 规则表驱动的镜像（而不是 Provider 继承体系）**
- 证据：`DownloadCandidates` / `DownloadCandidate{url, retry, timeouts}` + `HMCLDownloadProvider.MirrorRule` + `RegionUtils` + `DownloadSource`。
- 价值：新增镜像 = 加一行规则；"重试换源"是传输层内建行为，不需要业务代码感知；`fallback` 标志解决"镜像可能缺文件"。
- Rust 落地：`struct Candidate { url: Url, retries: u8, timeout: Duration }`，`Vec<Candidate>` 保序；`Fetch::get(candidates)` 内部循环，失败自动下一个。

#### ③ **不可变模型 + 保留原始 JSON（`rawJson`）**
- 证据：`GameInstanceManifest.rawJson`、`GameInstancePatch.rawJson`、`Library.TLauncher` 兼容适配器。
- 价值：面对"第三方启动器/加载器乱写字段"的现实，强类型模型必然丢字段。HMCL 的做法是：**已知字段强类型 + 未知字段原样保留 + 写回时合并**。
- Rust 落地：
  ```rust
  struct Manifest { #[serde(flatten)] known: Known, #[serde(flatten)] extra: serde_json::Map<String, Value> }
  ```
  或 `serde_json::Value` 作为 source of truth，视图按需解析（`#[serde(borrow)]`）。

#### ④ **布局（Layout）抽象 + 运行目录解析函数**
- 证据：`GameRepositoryLayout` 接口 + `DefaultGameRepositoryLayout` + `HMCLGameRepositoryLayout` + `computeRunDirectory()`。
- 价值：路径计算与 IO 分离，可单测；"版本隔离"变成纯函数；支持 MultiMC/官方两种布局；`hint=local` 让库可以落在实例内。
- Rust 落地：`trait Layout { fn instance_root(&self, id) -> PathBuf; fn library(&self, owner, lib) -> PathBuf; ... }`，`fn run_dir(instance, cfg) -> RunDir`。

#### ⑤ **可解释的进程退出与日志契约**
- 证据：`ExitWaiter`（用日志正则判 JVM_ERROR/APPLICATION_ERROR/SIGKILL）+ `ProcessListener` 三方法 + log4j 配置接管 + token 脱敏 + 环形缓冲。
- 价值：启动器最重要的体验是"崩了要能说清为什么"。HMCL 用**固定日志格式 + 关键词规则**做到了跨版本可用，而不是依赖游戏的退出码（游戏正常退出码几乎总是 0）。
- Rust 落地：`ExitReason { Normal, JvmInitFailed, CrashReport{path}, Killed, LauncherError }`；stdout/stderr 两个 `tokio::io::BufReader::lines()` + 一个 `wait()` task，三者用 `JoinSet` 汇合；日志行做 token 脱敏与 ANSI 清洗。

### 8.4 明确不建议照搬的 5 点

| 反模式 | 原因 | Rust 替代 |
|---|---|---|
| 任务图在 `execute()` 里动态生成 | 不可静态分析、不可持久化、不可断点续跑 | 显式 `Plan` 枚举（静态可构建），动态部分用 `Plan::Lazy(Box<dyn FnOnce>)` 并限制在叶子 |
| core 依赖 UI 属性（JavaFX Property） | 无法 headless、线程模型泄漏 | `watch::channel` / `ProgressSink` trait |
| `patches` 与"扁平字段"双写同一 JSON | 两份真相可能不一致，必须缓存 resolve 结果 | 只存 patches；扁平字段仅在导出给别的启动器时生成 |
| 80 行 `instanceof` 拼错误文案 | 错误类型爆炸、不可 i18n 化 | `thiserror` 错误码 + 一张 `code -> i18n key` 表 |
| 为每个镜像/格式写一个子类（历史上） | 类爆炸 | 数据驱动（规则表 + 注册表） |

---

## 9. 附录

### A. 关键文件索引（按阅读优先级）

| 优先级 | 文件 | 为什么读 |
|---|---|---|
| ★★★ | `HMCLCore/.../launch/DefaultLauncher.java` | 命令行拼装的完整规格（1055 行） |
| ★★★ | `HMCLCore/.../game/GameInstanceManifest.java` + `GameInstancePatch.java` | 版本模型与合成规则 |
| ★★★ | `HMCLCore/.../game/DefaultGameRepositorySnapshot.java` | 继承解析 + 环检测 + patch 合成 |
| ★★★ | `HMCLCore/.../task/FetchTask.java` | 下载器全部细节（重试/续传/缓存/并发） |
| ★★★ | `HMCL/src/.../game/LauncherHelper.java` | 启动流水线编排（1079 行） |
| ★★☆ | `HMCLCore/.../task/Task.java` / `AsyncTaskExecutor.java` | 任务模型 |
| ★★☆ | `HMCLCore/.../download/DownloadProvider.java` + `HMCL/.../HMCLDownloadProvider.java` | 版本列表抽象 + 镜像规则 |
| ★★☆ | `HMCLCore/.../download/forge/ForgeNewInstallTask.java` | Forge processors |
| ★★☆ | `HMCLCore/.../auth/OAuth.java` + `microsoft/MicrosoftService.java` | 微软登录全流程 |
| ★★☆ | `HMCLCore/.../game/GameRepositoryLayout.java` + `DefaultGameRepositoryLayout.java` | 路径抽象 |
| ★☆☆ | `HMCLCore/.../modpack/ModpackProvider.java` + `ModpackInstallTask.java` | 整合包策略 |
| ★☆☆ | `HMCL/src/.../setting/GameSettings.java` | 设置分层（Preset/Instance/Effective） |
| ★☆☆ | `HMCL/src/.../util/NativePatcher.java` | natives 平台补丁 |

### B. 术语对照

| HMCL 术语 | 含义 | nailong 建议命名 |
|---|---|---|
| instance / version | 一个可启动的版本实例（`versions/<id>`） | `Instance` |
| manifest | 版本 JSON（已解析为 record） | `Manifest` / `VersionJson` |
| patch | 组件对 manifest 的增量贡献 | `Patch` |
| component | 加载器/游戏本体等可安装单元 | `Component` |
| layout | 仓库路径计算策略 | `Layout` |
| run directory | 游戏工作目录（`--gameDir`） | `RunDir` |
| cache repository | 内容寻址缓存 | `ContentCache` |
| dependency manager | 仓库级服务聚合（下载+仓库+缓存） | `Context` / `Env` |
| stage | 进度阶段标识 | `StageId` |

### C. 待确认 / 风险点

1. `HMCLBoot/EntryPoint.java` 是占位类，真实 `EntryPoint` 在 `HMCL` 模块——依赖 shadowJar 的类覆盖顺序，属于**构建期魔法**，Rust 侧不要模仿（单一 `main.rs` 足够）。
2. `DefaultGameRepositoryLayout.getModpackConfigurationFile()` 返回 `modpack.cfg`，而 `DefaultGameInstance.getModpackConfigurationFile()` 返回 `modpack.json`——两处不一致，实际以实例方法为准（`modpack.json`）。移植时以自己的文件名为准并兼容读取两者。
3. `GameInstanceManifest.resolve()` 对 root+patches 实例会**忽略**已存储的扁平字段；如果第三方工具改写了扁平字段而没改 patches，改动会"消失"。这是双写设计的固有风险。
4. `FetchTask` 的断点续传依赖服务端支持 `Range` 且提供强 `ETag`/`Last-Modified`；BMCLAPI 的 `x-bmclapi-hash` 是私有约定，Rust 实现可作为可选优化。
5. 本文未深入：`addon/`（模组管理/远程仓库）、`schematic/`、`terracotta`（联机）、`upgrade/`（自更新）、`ui/`（JavaFX 界面）——这些与"启动核心"关系较弱，实现 nailong 时可后置。
