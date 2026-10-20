# Prism Launcher 架构研究：面向 nailong（Rust 轻量启动器）的设计输入

**研究对象**：`/tmp/ref/prism`，commit `77f2bee`（develop，Prism Launcher 11.x 时代；CMake 要求 Qt ≥ 6.8）。
**方法**：只读源码；对 `meta.prismlauncher.org/v1` 做了抽样（Minecraft 1.12.2 / 1.18.2 / 1.20.1 / 1.21.1、Forge、NeoForge、Fabric、Quilt），用于确认真实数据形态。未构建、未运行，因此运行时内存等不做实测断言。
**一句话结论**：Prism 是**元数据服务驱动的组件化启动器**，不是“官方 version.json 解析器 + inheritsFrom 继承链”。运行时输入是 `mmc-pack.json` 的扁平组件列表，每个组件是一份 Prism 组件格式 JSON（来自 meta 服务或 `patches/`），按列表顺序 `applyTo` 合并成 `LaunchProfile`，再据此下载、拼命令行、启动。官方 Mojang 格式的 `arguments` / `logging` 在客户端**完全不解析**；Forge/NeoForge 的 processors 也**不在启动器内执行**。这一判断决定了后面所有“可借鉴 / 必须避免”的取舍。

---

## 0. 核心结论（给决策者）

1. **模型极简、语义外包**：Prism 把“版本是什么”外包给 meta.prismlauncher.org；客户端只负责组件合并、规则过滤、下载、启动。生产代码中**没有任何路径调用 `MojangVersionFormat::versionFileFromJson`**（唯一调用点是 `tests/MojangVersionFormat_test.cpp`）；生产入口是 `OneSixVersionFormat::versionFileFromJson`（`launcher/minecraft/ProfileUtils.cpp:61`、`launcher/meta/JsonFormat.cpp:81`）。
2. **继承被“扁平列表 + 依赖解析”取代**：`inheritsFrom` 仅出现在 Technic 整合包解析（`launcher/modplatform/technic/TechnicPackProcessor.cpp:139`），不是通用机制。组件顺序就是覆盖顺序，组件间 `requires/conflicts` 由 `ComponentUpdateTask` 做启发式求解。
3. **加载器安装被“预构建 meta 包 + JVM 内包装器”取代**：Forge/NeoForge 的 meta 包把主类设为 `io.github.zekerzhayard.forgewrapper.installer.Main`，并把 installer 放进 `mavenFiles`；processors 由 ForgeWrapper 在游戏 JVM 内完成（第三方实现，非本仓库代码）。Fabric/Quilt 直接给 `KnotClient` + 库。OptiFine 没有专门支持。
4. **代价**：强依赖自建 meta 服务；丢失 `arguments`、`logging`、`os.arch`、`features` 等官方语义；依赖解析是“能跑就行”的启发式，硬编码了 LWJGL/Intermediary 的版本选择。
5. **对 nailong**：保留“组件可叠加、可禁用、可本地覆盖”的思想；把**官方 version.json 作为一等输入**（而不是先转成私有格式），meta 只作为 loader/优化叠加层；用纯函数 IR + 完整规则引擎 + 结构化并发（tokio）替换 Qt 的 model/signal 体系。

---

## 1. 实例与版本模型

### 1.1 类关系（谁拥有谁）

| 概念 | 位置 | 职责 | 关键点 |
|---|---|---|---|
| `BaseInstance` | `launcher/BaseInstance.h:85` | 实例公共基类：`instance.cfg`、运行状态、pre/post 命令、`RuntimeContext`、计时 | 依赖 `QObject`；菜单等 UI 由子类实现 |
| `MinecraftInstance` | `launcher/minecraft/MinecraftInstance.h:56` | 游戏实例：`gameRoot()`、`PackProfile`、各资源 FolderModel、启动任务、变量/环境 | 同时承担 UI 菜单（`MinecraftInstance.cpp:310` 明确 FIXME “move UI code out”） |
| `PackProfile` | `launcher/minecraft/PackProfile.h:59` | 组件列表（`QAbstractListModel`）、加载/保存 `mmc-pack.json`、解析依赖、缓存 `LaunchProfile` | UI model 与业务逻辑混在一起 |
| `Component` | `launcher/minecraft/Component.h:52` | 一个组件：`uid/version/enabled/important/dependencyOnly`；从 meta 或 `patches/<uid>.json` 装载 `VersionFile`；`applyTo` | `m_metaVersion` 与 `m_file` 二选一；本地 patch 优先 |
| `Meta::Version` | `launcher/meta/Version.h:36` | meta 包的一个版本：`requires/conflicts/recommended/volatile/sha256` + `VersionFilePtr data` | 列表项由 `VersionList` 管理 |
| `VersionFile` | `launcher/minecraft/VersionFile.h:60` | 一份组件 JSON 的**大杂烩结构体**（Mojang 字段 + Prism 扩展） | 解析器是 `friend`；字段随历史堆积 |
| `LaunchProfile` | `launcher/minecraft/LaunchProfile.h:42` | 合并结果：`mainClass/appletClass/minecraftArguments/addnJvmArguments/libraries/nativeLibraries/mavenFiles/agents/jarMods/mods/traits/tweakers/compatibleJava*` | 只做“后写覆盖/追加/去重”，不含规则求值 |

数据流：

```
mmc-pack.json
  └─ Component[]（有序）
       ├─ meta: <uid>/<version>.json  ─┐
       └─ patches/<uid>.json（本地覆盖）─┴─> VersionFile
                                            └─ applyTo(profile) 逐组件合并
                                                 └─ LaunchProfile
                                                      └─ 下载 / 规则过滤 / 拼 argv / 启动
```

### 1.2 `mmc-pack.json`：字段与语义

写入逻辑在 `PackProfile.cpp:106`（`componentToJsonV1`）/ `161`（`savePackProfile`），读取在 `139`（`componentFromJsonV1`）/ `188`（`loadPackProfile`）。

```json
{
  "formatVersion": 1,
  "components": [
    { "uid": "net.minecraft", "version": "1.20.1", "important": true },
    { "uid": "net.fabricmc.intermediary", "version": "1.20.1", "dependencyOnly": true },
    { "uid": "net.fabricmc.fabric-loader", "version": "0.16.10", "important": true }
  ]
}
```

> 注意：`jarMods` / `mainJar` / `+jvmArgs` 等字段属于**组件文件**（`patches/<uid>.json` 或 meta 版本 JSON），不属于 `mmc-pack.json` 的组件条目。

| 键 | 类型 | 语义 | 证据 |
|---|---|---|---|
| `formatVersion` | int | 必须等于 1，否则整份列表加载失败 | `PackProfile.cpp:207-210` |
| `components[].uid` | string | 组件 ID；只允许 `[A-Za-z0-9-_]+(\.[...])*`（组件文件解析时校验） | `OneSixVersionFormat.cpp:115-121` |
| `components[].version` | string | 组件版本；对 `net.minecraft` 即 MC 版本 | `Component.h:122-124` |
| `dependencyOnly` | bool | 由依赖解析自动加入，可被自动移除 | `Component.h:125` |
| `important` | bool | 不可删除（主组件/加载器） | `Component.h:126-127`、`Component::isRemovable` |
| `disabled` | bool | 禁用；`Component::isEnabled` 取反 | `Component.cpp:170-192` |
| `cachedVersion` / `cachedName` | string | 离线/UI 显示缓存，启动时由真实文件刷新 | `PackProfile.cpp:124-130`、`Component::updateCachedData` |
| `cachedRequires` / `cachedConflicts` | array | 依赖缓存，供未加载组件时做初步判断 | `PackProfile.cpp:131-132` |
| `cachedVolatile`（写入）/ `volatile`（读取） | bool | **键名不一致的 bug**：写入 `cachedVolatile`，读取 `volatile` | `PackProfile.cpp:133-135` vs `154` |

其他行为：

- 保存是 **5 秒防抖 + 析构时 `saveNow()`**（`PackProfile.cpp:78-99`、`236-296`）。
- 加载时重复 `uid` 直接忽略（`PackProfile.cpp:316-324`）。
- 组件顺序即优先级；`VersionFile::order` 字段仍被解析/序列化，但**没有任何排序调用**（`Component::getOrder()` 无调用者），不要被它误导。

### 1.3 `instance.cfg`：字段与“全局/实例”边界

实例发现：任何含 `instance.cfg` 的子目录（`InstanceList.cpp:486`）；装载：`InstanceList::loadInstance`（`InstanceList.cpp:706`）用 `INISettingsObject` 读 INI。

| 类别 | 典型键 | 说明 | 证据 |
|---|---|---|---|
| 元数据 | `name`、`iconKey`、`notes`、`uuid`、`linkedInstances`、`shortcuts` | 名称/图标/备注/联机实例 | `BaseInstance.cpp:75-88` |
| 计时 | `lastLaunchTime`、`totalTimePlayed`、`lastTimePlayed`、`CountGameTime` | 启动/游戏时长 | 同上 |
| 类型 | `InstanceType` | 只接受 `OneSix` | `InstanceList.cpp:711` |
| Java | `JavaPath`、`JvmArgs`、`OverrideJavaLocation`、`OverrideJavaArgs`、`AutomaticJava`、`JavaVersion`、`JavaArchitecture`、`JavaRealArchitecture`、`JavaVendor`、`JavaSignature` | 后四个是检测结果缓存，即使未 override 也会写入 | `MinecraftInstance.cpp:189-205`、`CheckJava.cpp:112-140` |
| 内存 | `OverrideMemory`、`MinMemAlloc`、`MaxMemAlloc`、`PermGen` | 由 override 门控 | `MinecraftInstance.cpp:214-218` |
| 窗口 | `OverrideWindow`、`LaunchMaximized`、`MinecraftWinWidth`、`MinecraftWinHeight` | | `MinecraftInstance.cpp:208-211` |
| 命令 | `OverrideCommands`、`PreLoadCommand`、`PreLaunchCommand`、`WrapperCommand`、`PostExitCommand` | 全局命令可被实例覆盖 | `BaseInstance.cpp:110-114` |
| 原生库 | `OverrideNativeWorkarounds`、`UseNativeOpenAL/GLFW/SDL`、`Custom*Path` | LWJGL 原生库替换 | `MinecraftInstance.cpp:220-227` |
| 性能 | `OverridePerformance`、`EnableFeralGamemode`、`EnableMangoHud`、`UseDiscreteGpu`、`UseZink` | Linux 为主 | `MinecraftInstance.cpp:229-234` |
| 其他 | `OverrideConsole`、`JoinServerOnLaunch`、`JoinServerOnLaunchAddress`、`JoinWorldOnLaunch`、`UseAccountForInstance`、`InstanceAccountId`、`ManagedPack*`、`Profiler` | | `MinecraftInstance.cpp:253-277`、`BaseInstance.cpp:121-140` |

**边界机制**：不是复制值，而是 `OverrideSetting` 在读取时按门控变量决定用实例值还是全局值（`settings/OverrideSetting.cpp:31-47`；`SettingsObject::registerOverride`）。`PassthroughSetting` 则始终读全局（如 `JavaVersion/JavaSignature`）。因此：

- 全局配置（`prismlauncher.cfg`）：`JavaPath`、`JvmArgs`、内存默认、代理、`MetaURLOverride`、并发数、控制台、更新通道、语言/主题。
- 实例配置（`instance.cfg`）：只存“实例特有/被 override 的值”；没 override 的键通常不落盘。
- 版本配置（`mmc-pack.json` + `patches/*.json`）：决定“启动什么”，与“怎么启动”分离。
- 共享资源（`assets/`、`libraries/`）在**数据目录**；实例内 `libraries/` 只存本地覆盖/自定义 jar。

### 1.4 “版本继承”到底怎么解析：答案是“没有”

- 全仓库没有通用 `inheritsFrom` 解析；仅 Technic 的 `bin/version.json` 读取该字段以确定 MC 版本（`TechnicPackProcessor.cpp:139-142`）。
- 取而代之的是**组件列表 + 依赖**：
  - 每个组件带 `requires`（如 Forge 包 `requires: [{uid:"net.minecraft", equals:"1.20.1"}]`）。
  - `ComponentUpdateTask::resolveDependencies`（`ComponentUpdateTask.cpp:463`）收集所有组件的 `m_cachedRequires`，合并同 uid 的约束；缺失的组件被插入到**第一个依赖者之前**（`ComponentUpdateTask.cpp:517-556`），并标记 `dependencyOnly=true`。
  - 版本选择优先级：`equals`（精确）> `suggests` > 硬编码兜底（`org.lwjgl→2.9.1`、`org.lwjgl3→3.1.2`、`intermediary/hashed→net.minecraft 的版本`）（`ComponentUpdateTask.cpp:529-548`）。
- 与文件继承的差异：
  - 文件继承是“子覆盖父”的树；组件模型是“有序平铺 + 约束求解”，能表达“Fabric + Intermediary + MC”这种多对多关系，也允许用户禁用/换版本。
  - 代价：没有“父版本”的隐式字段继承；所有字段必须由组件显式给出或在合并时保留前值。
- `patches/<uid>.json` **不是继承**，而是单个组件的本地覆盖：`Component::customize()` 把当前 `VersionFile` 写到 `patches/<uid>.json` 并清空 `m_metaVersion`（`Component.cpp:328-355`）；`loadComponent` 若发现该文件则优先使用（`ComponentUpdateTask.cpp:108-126`）。

**给 nailong**：扁平组件模型值得照搬；但建议内部用 `Component { uid, version, source, enabled, pinned }`，并额外提供“官方 version.json + 叠加层”的兼容路径（见 §9）。若需要导入旧 MultiMC/OneSix 实例，必须自己实现 `inheritsFrom` 链解析——Prism 已不做。

### 1.5 组件（loader）如何叠加

`PackProfile::getProfile()`（`PackProfile.cpp:965-977`）按列表顺序对每个组件调用 `VersionFile::applyTo`（`VersionFile.cpp:54-88`），`Component::applyTo` 会跳过禁用组件（`Component.cpp:85-97`）。

合并语义（`LaunchProfile.cpp`）：

| 字段 | 语义 | 证据 |
|---|---|---|
| `mainClass` / `appletClass` / `minecraftArguments` / `minecraftVersion` / `type` | 非空则覆盖（后者覆盖前者） | `applyString`，`LaunchProfile.cpp:58-88` |
| `mainJar` | 非空则替换 | `LaunchProfile.cpp:235-240` |
| `assets` | 仅 `net.minecraft` 组件可设置（硬编码保护） | `VersionFile.cpp:57-64` |
| `libraries` | 按 `group:artifact:classifier` 去重；版本更高者替换 | `findLibraryByName` + `Version` 比较，`LaunchProfile.cpp:127-200` |
| `nativeLibraries` | 同上，但只收 `isNative()` 的库 | `LaunchProfile.cpp:180-183` |
| `mavenFiles` | 追加、不去重（只下载不进 classpath） | `LaunchProfile.cpp:202-214` |
| `agents` | 追加；后续拼 `-javaagent` | `LaunchProfile.cpp:216-228` |
| `addnJvmArguments` | 追加 | `LaunchProfile.cpp:85-88` |
| `traits` | 并集 | `LaunchProfile.cpp:102-105` |
| `tweakers` | 已存在的先移除再追加（相当于“移到末尾”） | `LaunchProfile.cpp:107-120` |
| `jarMods` / `mods` | 追加 | `LaunchProfile.cpp:122-125`、`141-161` |
| `compatibleJavaMajors` | 追加 | `LaunchProfile.cpp:163-166` |
| `problemSeverity` | 取最大值 | `LaunchProfile.cpp:242-247` |

因此 loader 的叠加方式非常直接：**把自己追加在 MC 之后，用自己的 `mainClass`/库覆盖前者**。例如 Fabric 组件给 `net.fabricmc.loader.impl.launch.knot.KnotClient` 和一组 ASM/Mixin 库；Forge 组件给 ForgeWrapper 主类和一大堆库。`Component::KNOWN_MODLOADERS` 维护已知 loader 的冲突关系（Forge/NeoForge/Fabric/Quilt/LiteLoader，`Component.cpp:52-58`），`finalizeComponents` 只给“已知冲突”发 Warning（`ComponentUpdateTask.cpp:727-739`）。

**依赖变更的联动**：`PackProfile::setComponentVersion(uid, version, important=true)` 会设置 `UpdateActionImportantChanged`，触发 `performUpdateActions` 把依赖它的组件更新到 `equals` / `suggests` / “最新推荐兼容版本”（`ComponentUpdateTask.cpp:584-695`）。这是“换 MC 版本后 loader 跟着升级”的机制。

### 1.6 实例级 vs 全局：目录布局

| 路径 | 归属 | 说明 |
|---|---|---|
| `prismlauncher.cfg` | 全局 | `Application` 的 `INISettingsObject` |
| `accounts.json` | 全局 | 账户与 token；权限 0600（`AccountList.cpp:569`） |
| `metacache` | 全局 | `HttpMetaCache` 索引（ETag/MD5/age） |
| `meta/<uid>/index.json`、`meta/<uid>/<version>.json` | 全局 | meta 缓存 |
| `assets/indexes/<id>.json`、`assets/objects/<xx>/<hash>`、`assets/virtual/<id>/` | 全局 | 资源；`Application.cpp:445` 把 CWD 设为数据目录，代码里用相对 `assets/`、`libraries/` |
| `libraries/` | 全局 | Maven 布局；实例内 `libraries/` 为本地覆盖 |
| `java/` | 全局 | 自动下载的 JRE |
| `instances/<id>/instance.cfg` | 实例 | INI |
| `instances/<id>/mmc-pack.json` | 实例 | 组件列表 |
| `instances/<id>/patches/<uid>.json` | 实例 | 组件本地覆盖 |
| `instances/<id>/minecraft/`（或 `.minecraft/`） | 实例 | `gameRoot()`，工作目录 |
| `instances/<id>/natives/` | 实例 | 旧式 natives 解压目录（启动后 `finalize` 删除） |
| `instances/<id>/jarmods/` | 实例 | jar mod 文件 |
| `instances/<id>/libraries/` | 实例 | 本地库（custom jar、agent 等） |

`MinecraftInstance::gameRoot()` 优先 `minecraft/`，不存在则 `.minecraft/`（`MinecraftInstance.cpp:370-379`）。

---

## 2. 版本 JSON 解析

### 2.1 两条解析路径，一个结构体

- `MojangVersionFormat`：解析官方字段（`readVersionProperties`、`libraryFromJson`），供 `OneSixVersionFormat` 复用；**其 `versionFileFromJson` 仅测试使用**（`tests/MojangVersionFormat_test.cpp:16`）。
- `OneSixVersionFormat`：生产入口。既接受 Prism 组件格式，也“顺带”能读官方字段（因为复用 `readVersionProperties`/`libraryFromJson`），但不会读 `arguments`/`logging`。
- `VersionFile` 是两者共用的大结构体（`VersionFile.h:60-166`），因此字段来源混杂、语义靠注释区分。

### 2.2 官方字段的“读 / 不读”清单

| 官方字段 | 是否解析 | 说明 | 证据 |
|---|---|---|---|
| `id` / `mainClass` / `minecraftArguments` / `type` | 是 | 老式字符串参数 | `MojangVersionFormat.cpp:150-156` |
| `assets` / `assetIndex` | 是 | `assetIndex{id,sha1,size,totalSize,url}`；无 `assetIndex` 时用 `assets` 构造（`legacy` 有硬编码 URL） | `MojangVersionFormat.cpp:67-88`、`157-162`；`MojangDownloadInfo.h:44-65` |
| `releaseTime` / `time` | 是 | `timeFromS3Time` | `MojangVersionFormat.cpp:164-165` |
| `minimumLauncherVersion` | 是 | >18 只告警 | `MojangVersionFormat.cpp:167-176` |
| `compatibleJavaMajors` / `compatibleJavaName` | 是 | Prism meta 扩展，官方 JSON 没有 | `MojangVersionFormat.cpp:178-187` |
| `downloads`（client/server/…） | 是 | `mainJar` 的下载来源 | `MojangVersionFormat.cpp:189-195` |
| `libraries` | 是 | 见下 | `MojangVersionFormat.cpp:218-226` |
| **`arguments`** | **否** | 新式 `{game:[], jvm:[]}` 完全没读；由 meta 服务预扁平化成 `minecraftArguments` | 全仓库无 `"arguments"` 解析（仅 Technic/ATL 的包清单有同名字段） |
| **`logging`** | **否** | meta 里有，但 `VersionFile` 没有对应字段，解析器也不读；因此不会注入 `-Dlog4j.configurationFile` | `VersionFile.h`、`MojangVersionFormat.cpp:150-197`、`OneSixVersionFormat.cpp:84-297` |
| `javaVersion` | 否 | 用 Prism 的 `compatibleJava*` 替代 | — |
| `inheritsFrom` | 否 | 见 §1.4 | — |

> 抽样验证：`https://meta.prismlauncher.org/v1/net.minecraft/1.21.1.json` 同时包含 `minecraftArguments` 与 `logging`，但**没有** `arguments`；`+jvmArgs` 为空数组。说明“新式参数”在服务端就被压平了。1.12.2 / 1.18.2 的 meta 仍保留 `natives` 字段；1.21.1 没有。

### 2.3 `libraries` 与 rules

`MojangVersionFormat::libraryFromJson`（`MojangVersionFormat.cpp:288-333`）读取：

- `name`（Gradle 坐标，`GradleSpecifier` 解析 `group:artifact:version[:classifier][@ext]`）
- `url`（maven base）
- `extract.exclude`（解析进 `m_extractExcludes`，但**解压时未使用**）
- `natives`（`os -> classifier` 映射；有它才是 `isNative()`）
- `rules`
- `downloads.artifact` / `downloads.classifiers`

Prism 扩展（`OneSixVersionFormat::libraryFromJson`，`:55-64`）：`MMC-hint`、`MMC-absoluteUrl`、`MMC-filename`、`MMC-displayname`。

**规则求值**（`Rule.cpp:42-92`）：

```cpp
Rule::Action Rule::apply(const RuntimeContext& ctx) {
    if (m_os.has_value() && !ctx.classifierMatches(m_os->name)) return Defer;
    return m_action;
}
```

`Library::isActive`（`Library.cpp:239-257`）：

- 没有 rules → 允许；
- 有 rules → 初值 `Disallow`，按顺序应用，非 `Defer` 覆盖，**最后匹配者生效**；
- 若是 native，还要求 `getCompatibleNative(ctx)` 非空（当前 OS 有对应 classifier）。

**能力与缺口**：

| 维度 | Prism 支持 | 影响 |
|---|---|---|
| `os.name` | 是（`linux/windows/osx` 及 `osx-arm64` 之类组合） | 覆盖绝大多数库 |
| `os.version` | 解析但不使用（`Rule.h:56-60` 注释 “FIXME: unsupported”） | 老版本 Windows 特殊规则失效 |
| `os.arch` | **不支持**（`Rule` 没有 arch 字段；`Rule::fromJson` 只读 `os.name`） | x86/x64 规则会被当成“无 os 约束”→ 对所有架构生效 |
| `features` | **不支持**（参数级 `rules` 也没有） | `has_custom_resolution`、`is_demo_user`、quick play 等无法按官方语义处理 |
| 默认值 | 无 rules 允许；有 rules 默认拒绝 | 与 Mojang 语义一致 |

`RuntimeContext`（`RuntimeContext.h:26-68`）只含 `javaArchitecture`（“32/64”）、`javaRealArchitecture`（`amd64/aarch64/...`）和 `system`；`classifierMatches` 先试 `system + "-" + mappedArch`，再在“legacy arch”（x86/x86_64）时退回 `system`。

**给 nailong**：规则引擎必须补全 `arch`/`features`/`os.version`，并按“规则内所有谓词 AND、规则间最后匹配获胜、无规则默认允许”实现；这是官方 JSON 能直接消费的前提。

### 2.4 旧式 natives classifier vs 新式 natives 目录

**旧式（≤1.18 常见）**：库 JSON 带 `natives` 映射。

1. `Library::isNative()` = `m_nativeClassifiers` 非空（`Library.h:100`）。
2. `LaunchProfile::applyLibrary` 把它放进 `m_nativeLibraries`（`LaunchProfile.cpp:180-183`）。
3. `getCompatibleNative` 先找精确 `os-arch`（如 `linux-x86_64`），legacy 架构再退回 `os`（`linux`）（`Library.cpp:288-300`）。
4. classifier 名里的 `${arch}` 会展开成 32/64（`Library.cpp:166-188`、`getApplicableFiles:79-93`）。
5. `ExtractNatives` 把所有 native jar 解压到 `instance/natives/`；Java ≥8 时把 `.jnilib` 改名为 `.dylib`；启动加 `-Djava.library.path=<natives>`（`ExtractNatives.cpp:41-84`、`LauncherPartLaunch.cpp:113-117`）。
6. `LaunchProfile::getLibraryFiles` 按 `runtimeContext.javaArchitecture` 选择 `native32`/`native64`（`LaunchProfile.cpp:374-378`）。

**新式（1.19+ 常见）**：库 JSON **没有** `natives` 字段；native 以 classifier 形式出现在坐标里（如 `org.lwjgl:lwjgl:3.3.1:natives-linux`），并带 `rules`。

- 这些库 `isNative()==false`，因此进入**普通 classpath**；LWJGL 自己在运行时从 jar 中解压/加载 native。
- Prism 仍会设置 `-Djava.library.path` 指向 `instance/natives`（通常为空目录），无害。
- 抽样：1.21.1 meta 的 41 个库中 0 个带 `natives` 字段；1.12.2 有 2 个。

**extract.exclude 的坑**：`MojangVersionFormat` 解析并序列化了 `extract.exclude`（`:301-309`、`:350-358`），但 `ExtractNatives` 只做“全量解压 + jnilib 改名”，没有应用 exclude。结果可能把 `META-INF/MANIFEST.MF` 之类解压进 natives 目录。Rust 实现应过滤 `META-INF/` 及 exclude 列表。

### 2.5 assets 与 asset index

- 索引对象：`MojangAssetIndexInfo { id, url, sha1, size, totalSize }`（`MojangDownloadInfo.h:44-65`）；`assetIndex` 缺失时用 `assets` 值构造，`legacy` 有硬编码 URL（piston-meta / S3 旧地址）。
- 本地布局：`assets/indexes/<id>.json`、`assets/objects/<hash 前2位>/<hash>`（`AssetsUtils.cpp:161-189`、`AssetObject::getRelPath`）。
- `getAssetsDir(id, resourcesFolder)` 读索引：`virtual=true` → `assets/virtual/<id>`；`map_to_resources=true` → 实例 `resources/`；否则 `virtual/<id>`（`AssetsUtils.cpp:161-190`）。
- `reconstructAssets`：把 objects 里存在的文件复制到 virtual/resources；只复制缺失项；**不删除多余文件**（只打印 “Would remove”）（`AssetsUtils.cpp:193-265`）。
- 启动变量：`game_assets`（legacy 资源目录）、`assets_root`（数据目录 `assets/`）、`assets_index_name`（`MinecraftInstance.cpp:1083-1094`）。
- 完整性：`AssetObject::getDownloadAction` 只比较 `size`，不一致才下载，下载时加 SHA1 校验（`AssetsUtils.cpp:269-281`）；**没有启动前全量哈希校验**。索引本身走 `asset_indexes` 缓存并校验 SHA1（`AssetUpdateTask.cpp:21-48`）。
- 下载分两阶段：先索引，再 `AssetsIndex::getDownloadJob()` 批量下对象（`AssetsUtils.cpp:299-311`、`AssetUpdateTask.cpp:55-92`）；对象下载是直接写文件，不进 `HttpMetaCache`。

### 2.6 logging 配置（Prism 的缺口）

- meta 的 `logging` 形如：
  ```json
  { "argument": "-Dlog4j.configurationFile=${path}",
    "file": { "id": "client-1.12.xml", "sha1": "...", "size": 888, "url": "..." },
    "type": "log4j2-xml" }
  ```
- `VersionFile` 无字段、解析器不读，因此 Prism **不会**下载该 XML，也不会注入 JVM 参数；它只会在游戏/模组输出 log4j XML 时解析（`launch/LaunchTask.cpp:219-271`、`logs/LogParser.cpp`）。
- 对 nailong：这是低成本高收益的补全项——下载 `logging.client.file`（按 sha1）、把 `${path}` 替换为本地路径、加入 JVM args；同时保留“纯文本日志”解析路径。

### 2.7 旧版 jar / jar mod

- `VersionFile::jarMods` / `mods`（`VersionFile.h:137-141`）；组件可通过 `jarMods` 或旧式 `+jarMods` 声明（`OneSixVersionFormat.cpp:154-186`）。
- 启动前 `ModMinecraftJar`：把主 jar 与所有启用的 jar mod 合并到 `instance/bin/minecraft.jar`；**逆序**合并以尊重组件加载顺序；同名条目先到先得（`MMCZip.cpp:87-147`、`ModMinecraftJar.cpp:8-58`）。
- `LaunchProfile::getLibraryFiles` 在存在 jar mods 时，用 `bin/minecraft.jar` 替代主 jar 加入 classpath（`LaunchProfile.cpp:361-370`），这是唯一“覆盖主 jar”的方式。
- `finalize()` 删除临时 jar（`ModMinecraftJar.cpp:60-73`）。
- 自定义主 jar：`installCustomJar` 把文件放到实例 `libraries/`，并写一个 `mainJar` 指向 `custom:customjar:1` 的 patch（`PackProfile.cpp:851-904`）。
- 旧版 Forge 还需要 FML 库：`LegacyFMLLibrariesTask` 从 `LEGACY_FMLLIBS_BASE_URL` 下载并复制到实例 `lib/`（`minecraft/update/LegacyFMLLibrariesTask.cpp`）。

### 2.8 解析健壮性

- 全链路使用 `Result<T>`（`std::expected`）与 `TRY/TRY_INTO` 宏（`launcher/Result.h`），错误带上下文，不抛异常。
- `ProblemContainer` 收集 `ProblemSeverity::Warning/Error`，逐组件展示（`ProblemProvider.h`、`VersionFile.h:60`）。
- 但 `VersionFile` 的“部分成功”语义靠字段默认值，缺失关键字段（如 `mainJar` 无法推导）才报 Error（`OneSixVersionFormat.cpp:235-251`）。

---

## 3. 启动流程

### 3.1 总览

`LaunchController`（`launcher/LaunchController.cpp`，本身是 `Task`）负责：校验 JVM 参数 → 选账户 → 决定启动模式（Normal/Offline/Demo）→ 必要时刷新 token → `MinecraftInstance::createLaunchTask()` → 运行 `LaunchTask`。

`MinecraftInstance::createLaunchTask`（`MinecraftInstance.cpp:1156-1286`）按顺序 `appendStep`：

| # | Step（类） | 作用 | 证据 |
|---|---|---|---|
| 1 | `TextPrint` | 打印 Minecraft 目录 | `:1166` |
| 2 | `CreateGameFolders` | 建 `.minecraft` / `server-resource-packs` | `:1171` |
| 3 | `LookupServerAddress` | 解析 `--server` 的 SRV（仅端口 25565） | `:1186-1192` |
| 4 | `LaunchCommand`（Pre-Load） | 预加载命令，工作目录 gameRoot | `:1195-1199` |
| 5 | `TaskStepWrapper(MinecraftLoadAndCheck)` | **加载/解析组件元数据**（在线/离线） | `:1201-1205` |
| 6 | `AutoInstallJava` | 必要时自动选/装 JRE | `:1209` |
| 7 | `CheckJava` | 探测 Java、缓存签名/版本/架构 | `:1210` |
| 8 | `VerifyJavaInstall` | 校验 major/32 位内存 | `:1212` |
| 9 | `LaunchCommand`（Pre-Launch） | 启动前命令 | `:1216-1220` |
| 10 | `ClaimAccount` | 标记账户使用中（防刷新） | `:1224` |
| 11 | 更新任务（在线）：`FoldersTask`、`LibrariesTask`、`LegacyFMLLibrariesTask`、`AssetUpdateTask` | 下载库/资源 | `:1225-1227`、`:1142-1154` |
| 11' | `EnsureOfflineLibraries` | 离线时只检查文件是否存在 | `:1229` |
| 12 | `ModMinecraftJar` | 合并 jar mods | `:1234` |
| 13 | `ScanModFolders` | 扫描 mods/coremods | `:1239` |
| 14 | `EnsureAvailableMemory` | 内存告警 | `:1244` |
| 15 | `PrintInstanceInfo` | 打印实例/账户信息 | `:1249` |
| 16 | `ExtractNatives` | 解压旧式 native | `:1254` |
| 17 | `ReconstructAssets` | 重建 virtual/resources | `:1259` |
| 18 | `LauncherPartLaunch` | 真正启动 Java | `:1264-1268` |
| 19 | `LaunchCommand`（Post-Exit） | 退出后命令 | `:1272-1276` |
| 20 | `QuitAfterGameStop` | 退出启动器 | `:1280-1282` |

`MinecraftLoadAndCheck::executeTask`（`MinecraftLoadAndCheck.cpp:8-34`）：可选 `updateLatestMinecraft` → `PackProfile::reload(netmode)` → 等 `ComponentUpdateTask`；失败则整个启动失败。

`LaunchTask` 是顺序状态机：`executeTask` 启动第 0 步；`onStepFinished` 成功则下一步，失败则 `finalizeSteps(false)`；`finalizeSteps` **逆序**调用所有已执行 step 的 `finalize()` 做清理（`LaunchTask.cpp:74-126`）。

### 3.2 classpath 如何拼装

1. `MinecraftInstance::getClassPath()` → `profile->getLibraryFiles(runtimeContext, jars, nativeJars, getLocalLibraryPath(), binRoot())`（`MinecraftInstance.cpp:492-498`）。
2. `getLibraryFiles`（`LaunchProfile.cpp:348-379`）：
   - 遍历 `m_libraries`，每个 `Library::getApplicableFiles` 追加到 `jars`；
   - 主 jar：若有 jar mods 且 `addJarMods`，用 `bin/minecraft.jar`，否则主 jar 的实际路径；
   - 遍历 `m_nativeLibraries`，收集 native jars（含 32/64 分支）；
   - 按 `javaArchitecture` 选择 32 或 64 的 native 列表。
3. `Library::getApplicableFiles`（`Library.cpp:59-93`）用 `storagePrefix()` + `storageSuffix()` 算路径；`storageSuffix` = Gradle 坐标转路径（classifier 覆盖、`${arch}` 替换）；`isLocal()` 的库在 `overridePath`（实例 `libraries/` 或 `jarmods/`）下按文件名查找。
4. `LauncherPartLaunch::executeTask`（`LauncherPartLaunch.cpp:71-159`）：
   - `classPath.prepend(launcher jar)`，若 legacy 再前置 `NewLaunchLegacy.jar`；
   - Windows 用 `;`，其他用 `:`；
   - 参数：`-Djava.library.path=<natives>`、`-cp <classpath>`、主类 **`org.prismlauncher.EntryPoint`**。

**注意**：Prism **没有 `${classpath}` / `${classpath_separator}` / `${library_directory}` 的替换需求**，因为 classpath 是启动器原生拼的；`library_directory` 变量虽然存在于 `makeProfileVarMapping`（`MinecraftInstance.cpp:1093`），但仅供模板替换使用（官方 meta 的参数没有用到它）。主类不是从版本 JSON 直接传给 JVM，而是写进 launch script 交给 Java 侧 EntryPoint。

### 3.3 变量替换

`replaceTokensIn`（`MinecraftInstance.cpp:514-534`）用正则 `\$\{(.+)\}`（非贪婪）替换；未识别的变量会被**删除**（替换为空），不报错。

`makeProfileVarMapping`（`MinecraftInstance.cpp:1075-1096`）提供：

| 变量 | 值 |
|---|---|
| `profile_name` | 实例名 |
| `version_name` | `profile->getMinecraftVersion()` |
| `version_type` | `release` / `snapshot` |
| `game_directory` | `gameRoot()` 绝对路径 |
| `game_assets` | legacy 资源目录（`AssetsUtils::getAssetsDir`） |
| `assets_root` | 数据目录 `assets/` 绝对路径 |
| `assets_index_name` | asset index id |
| `library_directory` | `libraries/` 绝对路径 |

`processMinecraftArgs`（`MinecraftInstance.cpp:778-820`）额外注入：

- `auth_session` = `token:<accessToken>:<profileId>`
- `auth_access_token`、`auth_player_name`、`auth_uuid`、`user_properties`、`user_type`（`msa`/`offline`）
- Demo 模式追加 `--demo`
- quick play：`--quickPlayMultiplayer <addr:port>` / `--quickPlaySingleplayer <world>`，否则 `--server/--port`

### 3.4 JVM 参数与游戏参数模板

`MinecraftInstance::javaArguments()`（`MinecraftInstance.cpp:600-664`）：

1. `-Duser.language=en`
2. `extraArguments()`（**包含**用户 `JvmArgs`，以及 `+jvmArgs`、agent、原生库替换等追加项）：
   - 有 jar mods → `-Dfml.ignoreInvalidMinecraftCertificates=true`、`-Dfml.ignorePatchDiscrepancies=true`
   - `+jvmArgs`（做变量替换）
   - `-javaagent:<jar>[=arg]`
   - `-Dorg.lwjgl.openal.libname=...` / `glfw` / `sdl`（可选原生库替换）
3. macOS：`-Xdock:icon`、`-XstartOnFirstThread`（trait `FirstThreadOnMacOS`）
4. Windows：`-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump`
5. Linux：若装了 LWJGL2 且没有 `xrandr` → `-DLWJGL_DISABLE_XRANDR=true`
6. `-Xms/-Xmx`（min/max 反过来也处理）
7. Java <8：`-XX:PermSize`
8. Java ≥9 且启用 OnlineFixes：`--add-opens java.base/java.net=ALL-UNNAMED`

游戏参数：`profile->getMinecraftArguments().split(' ', SkipEmptyParts)` + `--tweakClass <tweaker>`（`MinecraftInstance.cpp:781-784`）。没有新式 `arguments.game` 的结构化处理。

### 3.5 工作目录、natives、log4j、进程

- 工作目录：`LauncherPartLaunch::setWorkingDirectory(gameRoot())`（`LauncherPartLaunch.cpp:212-215`，由 `createLaunchTask` 设置）。
- natives：`ExtractNatives` 解压到 `instance/natives`；启动任务结束（游戏退出或失败）后 `finalize()` **递归删除**（`ExtractNatives.cpp:87-93`）。这是“每次启动重新解压”的策略。
- log4j：无注入（见 §2.6）。
- 进程启动：`LoggedProcess`（继承 `QProcess`）设置 `setDetachable(true)`，stdout/stderr 按行切分（处理 `\r\n` 与跨块残留），可选 UTF-8 解码（`LoggedProcess.cpp:36-131`）。
- **启动脚本协议**（关键设计）：Java 侧 `EntryPoint` 从 stdin 逐行读 `key value`：
  - `mainClass`、`appletClass`、`param`（可多次）、`windowTitle/windowParams`、`launcherBrand/launcherVersion`、`userName/sessionId`、`traits`、`onlineFixes`、`launcher standard|legacy`
  - 启动器在进程进入 Running 后写入脚本，再等 `proceed()` 写 `launch\n`；`abort` 写 `abort\n`（`LauncherPartLaunch.cpp:161-239`、`libraries/launcher/org/prismlauncher/EntryPoint.java`）。
  - **好处**：access token 不出现在 `ps` 可见的 argv；legacy applet/窗口嵌入、OnlineFixes 都在 Java 侧统一处理。
  - **代价**：多一个 jar、多一层 Java 代码、启动路径更复杂；Rust 版可先直接传 `--accessToken`（并做日志脱敏），后续再决定是否加 shim。
- 退出监控：`LoggedProcess::on_exit` → `Finished/Crashed/Aborted`；`LauncherPartLaunch::on_state` 把非 0 退出码映射为 “Game crashed”；`CloseAfterLaunch` 在检测到 “Setting user” 日志后关启动器（`LauncherPartLaunch.cpp:54-65`、`161-210`）。
- 日志脱敏：`LaunchTask::censorPrivateInfo` 用 `createCensorFilterFromSession` 把 token/uuid/name 替换为占位符（`MinecraftInstance.cpp:1052-1073`、`LaunchTask.cpp:134-147`）。

---

## 4. 下载与元数据

### 4.1 meta 三层结构

`meta/Index`（`launcher/meta/Index.h`）、`VersionList`、`Version` 三级，本地文件：

- `index.json`（无 sha，首次必须联网；离线时可用本地缓存）
- `<uid>/index.json`（sha256 来自根 index 的 `packages[].sha256`）
- `<uid>/<version>.json`（sha256 来自列表项）

加载逻辑（`meta/BaseEntity.cpp:97-178`）：

1. 本地文件存在 → 读入、算 sha256；在线模式要求与期望 sha256 一致，否则视为不匹配。
2. 本地已加载且 hash 匹配 → 直接成功；否则 `NetJob` 下载。
3. 下载用 `Net::ApiRequest::makeCached` + `ChecksumValidator(Sha256)` + `ParsingValidator`（先解析成功才写盘）。
4. 成功后标 `LoadStatus::Remote`，`isLoaded()` 用 sha256 比较（`BaseEntity.cpp:77-81`）。

`Meta::JsonFormat::parseVersion`（`meta/JsonFormat.cpp:153-159`）把版本 JSON 交给 `OneSixVersionFormat::versionFileFromJson`（`JsonFormat.cpp:81`），因此 **meta 包就是组件格式**，不是官方格式。

`VersionList` 的 `ParentVersionRole` 把 `requires[uid=="net.minecraft"].equals` 暴露为“父版本”，用于 loader 版本筛选（`meta/VersionList.cpp:88-103`）；`getRecommendedForParent` / `getLatestForParent` 供 `UpdateActionLatestRecommendedCompatible` 使用（`meta/VersionList.cpp`）。

### 4.2 元数据源与镜像

- 默认 `META_URL = https://meta.prismlauncher.org/v1/`（`CMakeLists.txt:209`）。
- 运行时覆盖：`MetaURLOverride`（`BaseEntity::url`，`meta/BaseEntity.cpp:58-66`），设置页 `APIPage` 有占位符。
- 资源镜像：`ResourceURLOverride`（`AssetUpdateTask::resourceUrl`，`:115-121`）。
- 旧 FML 库镜像：`LegacyFMLLibsURLOverride`（`LegacyFMLLibrariesTask`）。
- 代理：`ProxyType/ProxyAddr/ProxyPort/ProxyUser/ProxyPass`，`Application::updateProxySettings`。
- **不直接访问 Mojang 的 version manifest**：Minecraft 版本、库 URL、asset index URL 全部来自 meta 包；只有 asset index 文件本身从 `assetIndex.url` 下载。因此 Prism 无法在 meta 服务不可用时启动新版本（离线缓存除外）。这是 Rust 版必须避免的单点。

### 4.3 网络栈

| 组件 | 职责 | 证据 |
|---|---|---|
| `Net::Request` | 单请求：GET/POST/自定义；重定向（最多 10 次，修正 `//`、相对 Location）；超时（默认 60s）；进度/速度/ETA；SSL 错误日志 | `net/Request.cpp:121-186`、`277-355` |
| `Net::Sink` | 输出端：`FileSink`（`PSaveFile` 原子写）、`ByteArraySink`、`MetaCacheSink` | `net/FileSink.cpp`、`MetaCacheSink.cpp` |
| `Net::Validator` | 下载中/结束时校验：`ChecksumValidator`（MD4/MD5/SHA1/SHA256/SHA512） | `net/ChecksumValidator.h` |
| `NetJob` | 批量下载，继承 `ConcurrentTask`；默认并发 6；失败自动重试 3 次（404 不重试）；可弹手动重试 | `net/NetJob.cpp:49-84`、`171-202`；`Application.cpp:669` |
| `HttpMetaCache` | URL→本地文件缓存索引：ETag、Last-Modified、MD5、max-age/eternal、base 路径；30s 批量落盘 | `net/HttpMetaCache.cpp:85-223`、`280` |
| `MetaCacheSink` | 条件请求（`If-None-Match`/`If-Modified-Since`）；写回 ETag/Last-Modified/MD5/age；`MakeEternal` 用于库 | `net/MetaCacheSink.cpp:57-130` |
| `Request::makeCached` | 组合 `MetaCacheSink` + MD5 校验 | `Request.cpp:539-547` |

**限流/重试**：429 时读 `Retry-After`（HTTP date 或秒），指数退避 `10 * 2^retry`，最多 4 次，延迟 >60s 直接失败（`Request.cpp:219-263`、`338-355`）。

**缓存 base 映射**（`Application.cpp:1032-1050`）：`asset_indexes`、`libraries`、`fmllibs`、`general`、各平台包缓存、`translations`、`meta`、`java`、`feed`。

**下载语义**：整文件下载，先写 `PSaveFile` 临时文件，校验通过才 `commit()`；**不支持断点续传/分块**（`FileSink.cpp:36-131`）。

### 4.4 资源完整性

- 库：`Library::getDownloads` 为每个 artifact 建 cache entry，`MakeEternal`，有 sha1 就加校验（`Library.cpp:108-228`）。
- 资源索引：sha1 校验；对象：size + sha1（仅下载时）。
- 启动前**没有**全量校验；只有 `EnsureOfflineLibraries` 检查 classpath 文件是否存在（`minecraft/launch/EnsureOfflineLibraries.cpp`）。
- 对 nailong：可提供“快速校验”（size+mtime）与“深度校验”（sha1，后台线程/并行）两档；大文件支持 Range 续传。

### 4.5 Java 检测与 JRE 下载

**检测**（`java/JavaChecker.cpp:51-205`）：

- 运行 `java -jar JavaCheck.jar`（jar 随程序分发，`JavaUtils::getJavaCheckPath()`），15 秒超时直接 kill。
- 解析 stdout 的 `key=value`：`os.arch`、`java.version`、`java.vendor`；`amd64/x86_64/aarch64/arm64/riscv64/ppc64*` 视为 64 位；输出 `mojangPlatform`（"32"/"64"）与 `realPlatform`。
- 失败/超时 → `Errored`；缺字段 → `ReturnedInvalidData`；成功 → `Valid`。

**缓存**（`launch/steps/CheckJava.cpp:44-140`）：`JavaSignature = sha1(lastModified + path)`；签名、版本、架构、vendor 存在实例设置；签名一致则跳过探测。探测后 `updateRuntimeContext()`，使规则/原生选择立即生效。

**查找路径**（`java/JavaUtils.cpp`，按平台四套 `FindJavaPaths`）：

- Windows：注册表（JavaSoft JRE/JDK、AdoptOpenJDK、Adoptium、Semeru、Zulu、Liberica、Microsoft）+ 默认路径 + `JAVA_HOME`。
- macOS：`/Library/Java/JavaVirtualMachines`、系统目录、SDKMAN、asdf、用户 Library、Minecraft/Prism 自带 JRE。
- Linux/BSD：默认 `java`、常见目录、`JAVA_HOME`、SDKMAN/asdf 等。

**自动安装**（`minecraft/launch/AutoInstallJava.cpp:62-242`）：

1. 全局 `AutomaticJavaSwitch` 开启且实例未强制指定 Java 时生效。
2. 若 `AutomaticJavaDownload` 关闭：只在本机 Java 列表里找 `compatibleJavaMajors` 匹配项。
3. 否则：从 meta `net.minecraft.java` 取 `java<major>` 版本，读 `runtimes[]`；按 `runtimeOS == SysInfo::getSupportedJavaArchitecture()`（如 `linux-x64`、`windows-x64`、`mac-os-arm64`）匹配。
4. `downloadType == manifest`：下载 manifest JSON，逐文件下载（`type=file/link/directory`，可执行位）；`archive`：下载压缩包并解压（去掉顶层目录）。
5. macOS 额外 `SymlinkTask`；失败清理目录并尝试下一个 major。
6. 成功后把 `JavaPath` 写成 `java/<runtimeName>/bin/java`，并置 `OverrideJavaLocation=true`。

**兼容性校验**：`VerifyJavaInstall` 检查 `compatibleJavaMajors`；32 位 + `MaxMemAlloc>2048` 告警；可用 `IgnoreJavaCompatibility` 跳过（`launch/steps/VerifyJavaInstall.cpp`）。

---

## 5. 任务模型

| 类 | 角色 | 关键行为 | 证据 |
|---|---|---|---|
| `Task` | 一切异步工作的基类，`QObject + QRunnable` | 状态 `Inactive/Running/Succeeded/Failed/AbortedByUser`；`start()` 一次性；`emitSucceeded/Failed/Aborted`；`progress/status/details/stepProgress` 信号；`canAbort()`/`abort()` | `tasks/Task.h:88-221`、`Task.cpp:84-160` |
| `SequentialTask` | 顺序执行 | 继承 `ConcurrentTask(max=1)`；子任务失败立即整体失败 | `tasks/SequentialTask.cpp:41-48` |
| `ConcurrentTask` | 并发池 | `m_queue/m_doing/m_done/m_failed`；完成后取下一个；进度按 `done/total`；`abort()` 取消队列并 abort 正在跑的；每个子任务发 `TaskStepProgress` | `tasks/ConcurrentTask.cpp:64-259` |
| `NetJob` | 下载专用并发 | 继承 `ConcurrentTask`；默认并发数来自设置（6）；失败重试 3 次（非 404）；可弹重试对话框 | `net/NetJob.cpp` |
| `TaskStepWrapper` | Task→LaunchStep 适配器 | 把任意 `Task` 包成启动步骤，转发进度/失败 | `launch/TaskStepWrapper.cpp` |
| `ComponentUpdateTask` | 组件加载 + 依赖解析 | 自己管理子任务列表、递归 `loadComponents`、直接改 `PackProfile` 内部 | `minecraft/ComponentUpdateTask.cpp` |

**取消语义**：`Task::abort()` 默认只在 `canAbort()` 且运行中时 `emitAborted()`；复合任务需重写并传播。`ConcurrentTask::abort` 会清空队列、逐个 abort 运行中的任务；`LaunchTask::abort` 只 abort 当前 step（`LaunchTask.cpp:175-202`）。

**进度上报**：`TaskStepProgress { uid, current, total, status, details, state }` 是并发任务的“每步子进度”，`ConcurrentTask` 用 `QHash<QUuid, shared_ptr<TaskStepProgress>>` 维护并通过 `stepProgress` 信号发给 UI（`tasks/Task.h:51-80`、`ConcurrentTask.cpp:166-240`）。

**错误处理**：字符串原因 + `failReason()`；`Task::emitFailed` 断言必须处于 Running（防止重复完成）；`Result<T>` 用于解析/文件操作，不用于任务。

**问题（Rust 不要照抄）**：

- `Task` 同时是 `QObject` 和 `QRunnable`，线程模型含糊；大量任务实际在同一事件循环里“异步”。
- 复合任务没有统一的结构化并发/取消层级；`ComponentUpdateTask` 自己维护 `remoteLoadStatusList`，并且 `TODO` 明确说应改成 `ConcurrentTask`（`ComponentUpdateTask.cpp:41-43`）。
- `ComponentUpdateTask` 直接访问 `d->m_profile->d->components`，注释自认 “This works directly with the PackProfile internals. It shouldn't!”（`:460-462`）。

---

## 6. 模组加载器安装与整合包导入

### 6.1 加载器 = meta 组件

`InstallLoaderDialog` 只是“选版本”的 UI，安装动作就是 `PackProfile::setComponentVersion(uid, version)`；组件内容来自 meta：

| Loader | uid | meta 里的主类/机制 | 证据 |
|---|---|---|---|
| Fabric | `net.fabricmc.fabric-loader` | `net.fabricmc.loader.impl.launch.knot.KnotClient`；依赖 `net.fabricmc.intermediary`；库含 ASM/Mixin | 抽样 meta；`InstallLoaderDialog.cpp:136-148` |
| Quilt | `org.quiltmc.quilt-loader` | `org.quiltmc.loader.impl.launch.knot.KnotClient`；同样依赖 intermediary | 抽样 meta |
| Forge（现代） | `net.minecraftforge` | 主类 `io.github.zekerzhayard.forgewrapper.installer.Main`；`mavenFiles` 含 `forge:...:installer` + 全部 processor 依赖；没有 `+jvmArgs` | 抽样 Forge 47.4.28 meta |
| NeoForge | `net.neoforged` | 同上，ForgeWrapper；`mavenFiles` 68 项（含 `neoforge:...:installer`） | 抽样 21.1.264 meta |
| Forge（旧） | `net.minecraftforge` | `net.minecraft.launchwrapper.Launch` + `+tweakers: ["...FMLTweaker"]` + `forge:...:universal` | 抽样 14.23.5.2860 meta |
| LiteLoader | `com.mumfrey.liteloader` | launchwrapper + tweaker（同类） | `Component.cpp:52-58`、`InstallLoaderDialog` |
| OptiFine | — | **没有专门支持**；只能作为 jar mod 或 Forge mod 安装 | 全仓库无 OptiFine 安装器（仅 ShaderPack 注释提到） |

`PackProfile::setComponentVersion` 对已存在组件先 `revert()` 再改版本；`important=true` 时触发依赖联动（`PackProfile.cpp:979-1007`）。版本列表按 `requires.net.minecraft == 当前 MC` 过滤（`VersionList::ParentVersionRole`）。

### 6.2 Forge 的 processors 到底在哪跑

**结论：不在启动器里跑。** 现代 Forge/NeoForge 的 meta 包将 `mainClass` 设为 ForgeWrapper，`mavenFiles` 放入 installer 及其依赖；据其入口与清单推断，ForgeWrapper 在**游戏 JVM 启动时**读取 installer、执行 processors、生成/校验 patched jars，然后再启动真正的 Forge 主类（第三方实现，细节不在本仓库）。启动器只负责把 installer 和依赖下载到 `libraries/`（mavenFiles 不进 classpath，但会下载）。

这带来：

- 优点：启动器实现极轻，不需要实现 Forge 的 install_profile / processors / 二进制 patch。
- 缺点：首次启动慢、依赖 Java 侧包装器、错误信息在游戏日志里、无法在启动前做完整性检查；且 ForgeWrapper 是第三方组件（Prism meta 使用的是带 `prism-...` 版本号的专用构建）。

**如果 nailong 要在启动器内实现 Forge 安装**（可选、重）：

1. 下载 `forge-<mc>-<ver>-installer.jar`，读 `install_profile.json`（`spec` 0/1）。
2. `spec 0`：`install` 字段含 `minecraft`、`minecraftJar`、`versionInfo`、`path`、`filePath`；`versionInfo` 内嵌完整版本 JSON；processor 链较少。
3. `spec 1`：`data` 的 `client/server` 映射（如 `{MINECRAFT_JAR}`、`{SIDE}`、`{ROOT}`、`{INSTALLER}`、`{LIBRARY_DIR}`），`processors[]` 每项含 `jar`、`classpath`、`args`、`outputs`（`{sha1,size}`）。
4. 逐个 processor：`java -cp <processor jar + classpath> <MainClass> <args...>`；输出写到 `libraries/`，校验 sha1；失败即回滚。
5. 生成 `version.json`（Forge 的版本描述），转成组件或直接作为 launch profile 的叠加层。
6. 需要处理 `client`/`server` 分支、`--offline`、重复安装、Java 版本要求。

对“轻量”目标，建议 v1 采用 Prism 的 ForgeWrapper 路线（或直接消费 Prism meta 的 Forge 包），把 processors 作为 v2 可选能力。

### 6.3 整合包格式与导入

`InstanceImportTask` 会自动识别 4 类 zip（`InstanceImportTask.cpp:129-230`、`InstanceImportTask.h:70-76`）；下表其余格式走各自入口，不属于 zip 自动识别：

| 格式 | 识别标志 | 处理 | 证据 |
|---|---|---|---|
| MultiMC/Prism | 任意 `instance.cfg` | 解压到 staging，读 `instance.cfg`，沿用 `mmc-pack.json`/`patches/`；重设名称/UUID/图标 | `InstanceImportTask.cpp:352-378`、`:164-167` |
| Modrinth `.mrpack` | `modrinth.index.json` | 解析 `files[{path,hashes,downloads,env}]`、`dependencies{minecraft,fabric-loader,quilt-loader,forge,neoforge}`；下载到 `gameRoot`；`overrides/`、`client-overrides/` 覆盖 | `ModrinthInstanceCreationTask.cpp:90-230` |
| CurseForge | `manifest.json` | `minecraft.version` + `modLoaders[{id,primary}]` + `files[{projectID,fileID,required}]`；经 Flame API 解析下载 URL；`overrides/` | `FlameInstanceCreationTask.cpp:100-260`、`PackManifest` |
| Technic | `bin/modpack.jar` 或 `bin/version.json` | 读 `version.json` 的 `inheritsFrom` 定 MC 版本；解压 `mods/`、`config/` 等 | `TechnicPackProcessor.cpp:35-160` |
| ATLauncher / FTB / ImportFTB | UI 单独入口 | 各自的 `*PackInstallTask`，不在 zip 自动识别里 | `modplatform/atlauncher`、`ftb`、`import_ftb` |
| Packwiz | — | **不是导入格式**；仅作为 mod 索引元数据（`.pw.toml`）的读写 | `modplatform/packwiz/Packwiz.cpp`、`minecraft/mod/MetadataHandler.h` |

**覆盖更新**（Modrinth/CurseForge）：用 `ManagedPack*` 字段找旧实例；读旧 index 计算差异，删除旧 overrides、跳过相同文件、保留存档（`ModrinthInstanceCreationTask.cpp:90-190`、`FlameInstanceCreationTask.cpp:100-200`）。这套逻辑复杂且容易出路径安全问题；11.1.1 的发布说明明确修复了 CurseForge overrides 的路径穿越（#6183）。Rust 实现必须在解压每个条目时做 canonicalize + 前缀校验（含 symlink 解析）。

**mmcpack 格式**：本版本没有独立的 `mmcpack` 清单；Prism 导出就是“整个实例目录打 zip”，导入靠 `instance.cfg` 识别。因此不存在需要兼容的 mmcpack 索引。

---

## 7. 账户

### 7.1 微软 OAuth：device code 与 loopback 两条路

`AuthFlow`（`minecraft/auth/AuthFlow.cpp:20-154`）按 `Action` 组装步骤链：

- `Action::DeviceCode`：`MSADeviceCodeStep` → Xbox → XSTS → Minecraft → entitlements → profile → skin
- `Action::Login`：`MSAStep`（授权码流，浏览器 + 回调）→ 同上
- `Action::Refresh`：`MSAStep(silent=true)` → 同上

| 步骤 | 作用 | 端点/细节 | 证据 |
|---|---|---|---|
| `MSADeviceCodeStep` | 设备码登录 | `POST /consumers/oauth2/v2.0/devicecode`（scope `XboxLive.SignIn XboxLive.offline_access`），轮询 `/token`；处理 `authorization_pending` / `slow_down` / 超时；`interval` 递增 | `MSADeviceCodeStep.cpp:58-272` |
| `MSAStep` | 授权码/刷新 | `QOAuth2AuthorizationCodeFlow`；`authorize` + `token`；普通安装且已注册 scheme 时用 `prismlauncher://oauth/microsoft` 自定义 scheme，便携版/未注册时回退 `QOAuthHttpServerReplyHandler`（loopback）；刷新用 `refresh_token` | `MSAStep.cpp:114-209` |
| `XboxUserStep` | Xbox Live 用户令牌 | `https://user.auth.xboxlive.com/user/authenticate`，`x-xbl-contract-version: 1` | `XboxUserStep.cpp:32-80` |
| `XboxAuthorizationStep` | XSTS 授权 | `https://xsts.auth.xboxlive.com/xsts/authorize`，relying party `rp://api.minecraftservices.com/`；解析 `XErr` 并给出人话错误（无 Xbox 档案/地区限制/未成年/封禁等） | `XboxAuthorizationStep.cpp:40-170` |
| `LauncherLoginStep` | 换 Minecraft access token | `POST https://api.minecraftservices.com/launcher/login`，body `{"xtoken":"XBL3.0 x=<uhs>;<token>","platform":"PC_LAUNCHER"}` | `LauncherLoginStep.cpp:20-75` |
| `EntitlementsStep` | 是否拥有游戏 | `GET /entitlements/license?requestId=...` | `EntitlementsStep.cpp:28` |
| `MinecraftProfileStep` | 档案（uuid/name/skin） | `GET /minecraft/profile`；404 视为“无档案但成功” | `MinecraftProfileStep.cpp:19-80` |
| `GetSkinStep` | 皮肤 PNG | 下载 `profile.skin.url` | `GetSkinStep.cpp:12-37` |

### 7.2 token 刷新与状态机

- `AccountData` 持有 `msaToken`、`userToken`、`mojangservicesToken`（XSTS）、`yggdrasilToken`（Minecraft access token），以及 `Validity`（None/Assumed/Certain）（`AccountData.h:49-124`）。
- `MinecraftAccount::shouldRefresh()`（`:207-240`）：
  - 游戏占用中不刷新（会破坏会话）；
  - `Validity::Assumed` 立即刷新；
  - 过期时间缺失时按“签发 + 24h”；
  - 剩余 <12h 刷新。
- `AuthFlow::changeState` 把 `AccountTaskState` 映射为 `AccountState`（Online/Offline/Errored/Expired/Disabled/Gone），并区分“软失败/硬失败/网络离线”（`AuthFlow.cpp:81-147`）。
- `AccountList` 启动时 `fillQueue()` 把默认账户排到最前，后台串行刷新（`AccountList.cpp:560-640`）。

### 7.3 离线账户

`MinecraftAccount::createOffline`（`:77-90`）：

- `type=Offline`，access token `"0"`，`yggdrasilToken.validity=Certain`；
- UUID = Java `UUID.nameUUIDFromBytes("OfflinePlayer:<name>")` 的 v3 实现（MD5 + 版本/变体位，`:282-297`）；
- `fillSession` 生成 `session = "token:0:<uuid>"`，`user_type = "offline"`（`:242-259`）。

### 7.4 皮肤

- 登录链最后拉取皮肤 PNG（`GetSkinStep`），存 `AccountData.minecraftProfile.skin.data`；
- `getFace()` 从 8×8 头部区域拼脸（`MinecraftAccount.cpp:102-114`）；
- `SkinList/SkinModel` 管理本地皮肤库与预览（`minecraft/skins/`），与启动无关。

### 7.5 安全现状

- token 存 `accounts.json`，文件权限 0600（`AccountList.cpp:569`），但**明文**；
- 启动时通过 launch script（stdin）传给 Java，不落 argv；
- 日志用 `censorPrivateInfo` 脱敏；
- 对 nailong：refresh token 放 OS keychain（Windows Credential Manager / macOS Keychain / libsecret），access token 仅内存；若不做 Java shim，至少不要把 token 放进会被日志打印的 argv。

---

## 8. “轻量”维度：Prism 的负担与可借鉴点

### 8.1 体积与依赖（可量化）

**代码量**（`launcher/`，C++）：

| 子系统 | 文件数 | LOC |
|---|---|---|
| `ui/` | 303 | 44,988 |
| `minecraft/` | 162 | 24,840 |
| `modplatform/` | 74 | 12,452 |
| `net/` | 26 | 3,042 |
| `java/` | 18 | 2,138 |
| `launch/` | 20 | 1,567 |
| `meta/` | 10 | 1,424 |
| `settings/` | 12 | 1,356 |
| `tasks/` | 8 | 1,043 |
| **合计** | **781** | **116,775** |

UI 占 38.5%，是“重”的主要来源。

- Qt 模块（`CMakeLists.txt:302-322`）：Core、Concurrent、Network、**NetworkAuth**、OpenGL、Test、Widgets、Xml（DBus 可选）；第三方：libarchive、cmark、toml++、qrencode、zlib（`vcpkg.json`）。
- 官方 11.1.1 发布产物大小（GitHub API）：Linux x86_64 AppImage **107,577,656 B ≈ 102.6 MiB**；Linux 便携 tar.gz ≈ 61.0 MiB；Windows MSVC 便携 zip ≈ 19.5 MiB；Windows 安装器 ≈ 22.9 MiB；macOS zip ≈ 41.6 MiB。
- 这些是“压缩后”的体积；解压后的 Qt 插件/翻译/图标资源更大。对“轻量”目标，Qt Widgets + NetworkAuth 是最主要的不可压缩负担。

### 8.2 启动与常驻开销（结构性）

`Application` 启动即构造：`IconList`、`ThemeManager`、`InstanceList`（同步加载所有实例的 `instance.cfg`）、`AccountList`（读 `accounts.json` 并排队刷新）、`HttpMetaCache`、`TranslationsModel`（`Application.cpp:960-1060`）；`Meta::Index` 是首次使用时惰性创建（`Application.cpp:1874-1877`）。每个实例在构造时创建 `PackProfile`（`MinecraftInstance.cpp:170-174`），`FolderModel`/`QFileSystemWatcher` 则按需创建。因此：

- 冷启动成本随实例数线性增长；
- 常驻内存包含 Qt 元对象、图标缓存、翻译、所有实例模型；
- 无 headless 模式：`LaunchController` 直接依赖 `QMessageBox/ProgressDialog/QWidget`，无法在 CLI/服务端复用。

> 未在本环境实测 RSS；以上是结构性判断，不作为数字结论。

### 8.3 模块耦合清单（Rust 必须避免的形态）

| 耦合 | 证据 | 后果 |
|---|---|---|
| 全局单例 `APPLICATION` | `Application.h`、几乎每个 `.cpp` | 难测试、难嵌入、初始化顺序隐式 |
| UI 逻辑在核心类 | `MinecraftInstance::populateLaunchMenu`（`:310`） | 核心无法脱离 GUI |
| `PackProfile : QAbstractListModel` | `PackProfile.h:59` | 业务模型绑定 Qt model/view |
| `ComponentUpdateTask` 直接改 `PackProfile::d->components` | `ComponentUpdateTask.cpp:460-462`、`:552` | 事务性差、难并发、难回滚 |
| `VersionFile` 万能结构体 + `friend` 解析器 | `VersionFile.h:60`、`:61-62` | 字段语义靠注释；新增格式易破 |
| `Library::limitedCopy` 不完整复制 | `Library.h:66-80`（漏 `m_displayname`、`m_hasExcludes`、`applyRules`） | 微妙的规则/显示丢失 |
| `Rule` 只按 `os.name` 生效（arch/features 缺失） | `Rule.h:56-65` | 官方 JSON 语义丢失 |
| `order` 死字段 | `Component::getOrder()` 无调用者 | 误导读代码/写包的人 |
| 启动脚本/JVM 侧 shim | `LauncherPartLaunch` + `libraries/launcher` | 多一层 Java 代码与协议 |
| 任务即 QObject | `Task.h:88` | 线程/生命周期含糊 |

**具体 bug（说明“不要逐行移植”）**：

- `PackProfile` 写 `cachedVolatile`、读 `volatile`（`:133-135` vs `:154`）。
- `OneSixVersionFormat::versionFileToJson` 写 `mods` 时遍历的是 `patch->jarMods`（`:352-358`）。
- `LaunchProfile::applyMods` 在循环里 `return` 而不是 `continue`（`:141-161`），只处理第一个 mod。
- `extract.exclude` 解析但未应用（§2.4）。
- `Rule` 的 `os.version` 注释 “FIXME: unsupported”。

### 8.4 值得借鉴的部分

1. **组件化 + 本地 patch 覆盖**：用户可禁用/改版本/写自定义 JSON，不被 meta 锁死。
2. **`requires/conflicts` 的轻量约束模型**：对 loader 生态足够，且能被 UI 解释。
3. **按组件顺序合并 + 库去重取高版本**：简单、可预测，能处理 Forge/Fabric 的库覆盖。
4. **meta 三层缓存 + sha256 + 条件请求**：离线可用、增量更新、防篡改。
5. **`Task` 的进度/状态/取消契约**：概念清晰，适合映射到 Rust 的 future + 事件。
6. **下载原子写 + 校验后提交**：避免半文件。
7. **启动脚本/日志脱敏**：token 不进 argv 的思路值得保留（可用 Rust 侧 shim 或 Java 侧）。
8. **规则“最后匹配获胜”**：与 Mojang 一致，容易实现。
9. **`ForgeWrapper` 的务实**：如果不想实现 processors，这是可接受的折中（但要接受首次启动慢）。

---

## 9. 对 nailong 的具体设计建议

### 9.1 模块划分（建议）

```
nailong/
  crates/
    nailong-core/      # 纯逻辑，无 UI/网络实现依赖
      model.rs         # Instance, Component, VersionFile, Library, Rule, ResolvedProfile
      resolve.rs       # 组件合并 + 依赖求解（纯函数）
      rules.rs         # os/arch/features 规则引擎
      args.rs          # 官方 arguments 合并 + 变量替换
      launch.rs        # LaunchPlan（argv/cwd/env/classpath/natives）
    nailong-net/       # reqwest + 缓存 + 校验 + 并发/重试
    nailong-meta/      # Mojang piston-meta + Prism meta 叠加
    nailong-auth/      # MSA device code / refresh / offline
    nailong-java/      # 检测 / 下载 / 选择
    nailong-loader/    # Fabric/Quilt/Forge/NeoForge 安装策略
    nailong-pack/      # mrpack / CurseForge / MMC 导入导出
    nailong-cli/       # headless：launch/list/install/export
    nailong-gui/       # Tauri/Slint/egui，仅依赖 core 的事件/命令
```

硬规则：`nailong-core` 不依赖 `tokio`、不依赖 UI、不做 I/O；输入是 `InstanceLayout` + `Vec<VersionFile>`，输出是 `LaunchPlan`。这样可用 golden test 锁死行为。

### 9.2 数据模型（Rust 草案）

```rust
struct Instance { id: String, root: PathBuf, config: InstanceConfig, components: Vec<Component> }

struct Component {
    uid: String,
    version: String,
    enabled: bool,
    important: bool,
    dependency_only: bool,
    source: ComponentSource, // Meta | LocalPatch(PathBuf)
}

struct VersionFile {
    format_version: u32,
    uid: String, name: String, version: String,
    main_class: Option<String>, applet_class: Option<String>,
    // 官方新式 + 旧式都保留
    arguments: Option<Arguments>,          // { game: Vec<Arg>, jvm: Vec<Arg> }
    minecraft_arguments: Option<String>,   // legacy
    logging: Option<Logging>,
    libraries: Vec<Library>,
    maven_files: Vec<Library>,
    jar_mods: Vec<Library>,
    mods: Vec<Library>,
    agents: Vec<Agent>,
    traits: BTreeSet<String>,
    requires: Vec<Require>, conflicts: Vec<Require>,
    asset_index: Option<AssetIndex>,
    downloads: BTreeMap<String, DownloadInfo>,
    java_majors: Vec<u32>, java_name: Option<String>,
    order: i32, // 仅兼容，不参与排序
}

enum Arg { Literal(String), Conditional { rules: Vec<Rule>, value: Vec<String> } }

struct Rule { action: Action, os: Option<OsRule>, features: BTreeMap<String, bool> }
struct OsRule { name: Option<String>, arch: Option<String>, version: Option<String> }
```

关键：`VersionFile` 同时保留 `arguments` 与 `minecraftArguments`，由 `args.rs` 决定“新式优先、旧式回退”，不要像 Prism 那样在解析期丢掉新式。

### 9.3 解析/合并流水线（伪代码）

```rust
fn resolve(instance: &Instance, docs: &[VersionFile], ctx: &RuleContext) -> Result<LaunchPlan> {
    // 1. 依赖求解：保证每个 requires 有组件；版本选择 exact > suggests > policy
    let components = solve_dependencies(&instance.components, docs)?;

    // 2. 顺序合并
    let mut p = Profile::default();
    for c in components.iter().filter(|c| c.enabled) {
        p.merge(&docs[c.uid], ctx)?; // 标量后写覆盖；列表追加；库去重取高版本
    }

    // 3. 规则过滤
    let libs = p.libraries.iter().filter(|l| l.is_active(ctx)).cloned().collect();
    let natives = libs.iter().filter(|l| l.is_old_style_native()).cloned().collect();

    // 4. 参数
    let game_args = build_game_args(&p, ctx, &auth);
    let jvm_args  = build_jvm_args(&p, ctx, &memory, &logging);

    // 5. classpath / natives 解压计划 / 资源下载计划
    Ok(LaunchPlan { argv, cwd, env, classpath, native_dir, downloads, ... })
}
```

**依赖求解**：建议用“约束图 + 最小变更”而不是 Prism 的递归重载：

- 节点 = uid；边 = requires；约束 = `=version` / `suggests` / `any`。
- 先做环检测；对 `=version` 冲突直接报错；对 `suggests` 选择“满足所有约束的最高版本”。
- 新增依赖插入到第一个依赖者之前（保持 Prism 的语义），保证覆盖顺序。
- 记录“为什么选这个版本”，UI 可解释。

### 9.4 规则引擎（必须比 Prism 完整）

```rust
fn matches(rule: &Rule, ctx: &RuleContext) -> Option<Action> {
    if let Some(os) = &rule.os {
        if let Some(n) = &os.name { if n != ctx.os_name { return None; } }
        if let Some(a) = &os.arch { if a != ctx.os_arch { return None; } }
        if let Some(v) = &os.version { if !version_req_matches(v, &ctx.os_version) { return None; } }
    }
    for (k, v) in &rule.features { if ctx.features.get(k) != Some(v) { return None; } }
    Some(rule.action)
}
// 无 rules => Allow；有 rules => 初始 Disallow，最后一个 Some 生效
```

`RuleContext` 至少包含：`os_name`（`windows/osx/linux`）、`os_arch`（`x86/x86_64/arm64`）、`os_version`、`features`（`is_demo_user`、`has_custom_resolution`、`has_quick_plays_support`、`is_quick_play_singleplayer/multiplayer/realms`）、`java_major`（部分库用）。

### 9.5 启动计划与安全

- `LaunchPlan` 只描述“要执行什么”，不执行；`nailong-cli`/GUI 各自执行。
- argv 用 `Vec<OsString>`，绝不拼 shell；工作目录、环境变量显式传递。
- token 处理两档（Minecraft 没有“从环境变量读 token”的官方入口）：
  1. 最简（v1）：放 argv，`ps` 可见；必须对日志/崩溃报告/错误弹窗做脱敏，并明确告知风险。
  2. 完整（v2）：启动器把 launch script 写入 Java shim 的 stdin，shim 用反射调用游戏主类（Prism `StandardLauncher` 的做法），token 全程不进入任何进程的 argv；成本是一个几十 KB 的 jar + 一个 Java 文件。也可直接复用 Prism 的 launcher-part jar（注意 GPL 许可）。
- 日志：结构化 + 行级脱敏（token、uuid、session、玩家名）；不要把完整 URL（含签名参数）写日志。
- 文件安全：解压 zip 时 `canonicalize` 每个目标路径，拒绝 `..`/绝对路径/符号链接逃逸；限制总大小/文件数；覆盖前校验（参考 11.1.1 的 CurseForge 路径穿越修复）。

### 9.6 下载/缓存

- 内容寻址：`objects/<sha1[0..2]>/<sha1>`；Maven：`libraries/<group path>/<artifact>/<version>/<file>`。
- 缓存索引：SQLite（`url, etag, last_modified, sha1, size, fetched_at, max_age`），支持条件请求；或 JSON + 定期压缩。
- 并发：全局 `Semaphore`（默认 6–8），每个大文件可分块（Range）+ 断点续传；小文件整文件。
- 重试：仅对网络错误/5xx/429；4xx 不重试；指数退避 + jitter；尊重 `Retry-After`。
- 校验：下载后强校验；启动前可选“快速校验”（size+mtime）或“深度校验”（sha1，后台）。
- 镜像：Mojang 官方 + BMCLAPI 等镜像可配置；meta 源可配置；不要像 Prism 那样把 meta 作为唯一来源。

### 9.7 Java

- 检测：优先 `java -XshowSettings:properties -version` 解析 `java.version/java.vendor/os.arch`（stderr），失败再 `java -version`；不必分发 JavaCheck.jar。
- 记录 `path + mtime + size` 作为签名，避免每次启动探测。
- 选择：`compatibleJavaMajors`（官方 `javaVersion.majorVersion` 或 Prism meta 扩展）+ 已安装 JDK 列表 + 自动下载（Adoptium API 或 Prism `net.minecraft.java` 数据）。
- 32 位 + 大内存、Java 版本不匹配给明确 warning，不硬失败。

### 9.8 加载器策略（建议分阶段）

| 阶段 | 策略 | 成本 | 适用 |
|---|---|---|---|
| v1 | Fabric/Quilt：直接拼 meta/官方 loader JSON（KnotClient + libs） | 低 | 大多数轻量用户 |
| v1 | Forge/NeoForge：消费 Prism meta 的 ForgeWrapper 包（或自建同形态包） | 低 | 快速可用 |
| v2 | 自建 Forge processors 安装器（install_profile + processors + outputs 校验） | 高 | 不想依赖第三方 wrapper/服务 |
| v2 | OptiFine：作为 jar mod / 提示用户用 Forge 版 | 低 | 兼容 |

### 9.9 整合包

- 先做 mrpack（格式最简单、hash 完整、生态最广）。
- CurseForge 需要 API key（Prism 在构建期注入 `FLAME_API_KEY`），Rust 版应让用户配置 key 或走第三方镜像；不要硬编码。
- MMC/Prism zip：直接解压 + 读 `instance.cfg`/`mmc-pack.json`；这也是与 Prism 互操作的最低成本方式。
- 导入事务：`staging/` 临时目录 → 校验 → 原子 rename 到 `instances/<id>`；失败清理；更新时保留 `saves/`、`options.txt` 等用户数据。

### 9.10 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M0 | 模型 + 官方 JSON 解析 + 规则/参数 golden tests | 1.8.9/1.12.2/1.20.1/1.21.1 的 argv/classpath 快照 |
| M1 | 下载 + 缓存 + 离线启动 vanilla | 断网可启动已缓存实例 |
| M2 | MSA device code + 在线启动 | 正版登录、token 刷新 |
| M3 | Fabric/Quilt 组件 | 一键装 loader 并启动 |
| M4 | Forge/NeoForge（wrapper 或 processors） | 首次启动可用 |
| M5 | mrpack/MMC 导入 | 导入即玩 |
| M6 | GUI | 实例列表/设置/日志 |
| M7 | Java 自动下载、镜像、深度校验 | 无 Java 环境可一键跑 |

---

## 10. 证据索引（按主题）

| 主题 | 关键文件/函数 |
|---|---|
| 组件列表读写 | `launcher/minecraft/PackProfile.cpp`（`componentToJsonV1`、`loadPackProfile`、`getProfile`） |
| 组件模型 | `launcher/minecraft/Component.h/.cpp`（`applyTo`、`customize`、`updateCachedData`） |
| 依赖求解 | `launcher/minecraft/ComponentUpdateTask.cpp`（`loadComponents`、`resolveDependencies`、`performUpdateActions`） |
| 合并语义 | `launcher/minecraft/VersionFile.cpp`（`applyTo`）、`LaunchProfile.cpp`（`applyLibrary`、`getLibraryFiles`） |
| 官方/组件解析 | `launcher/minecraft/MojangVersionFormat.cpp`、`OneSixVersionFormat.cpp`、`ProfileUtils.cpp` |
| 规则 | `launcher/minecraft/Rule.cpp`、`Library.cpp`（`isActive`、`getCompatibleNative`） |
| 资源 | `launcher/minecraft/AssetsUtils.cpp`、`minecraft/update/AssetUpdateTask.cpp` |
| 启动步骤 | `launcher/minecraft/MinecraftInstance.cpp`（`createLaunchTask`、`javaArguments`、`processMinecraftArgs`） |
| 启动执行 | `launcher/minecraft/launch/LauncherPartLaunch.cpp`、`launcher/launch/LaunchTask.cpp` |
| 元数据 | `launcher/meta/*`（`Index`、`VersionList`、`Version`、`JsonFormat`、`BaseEntity`） |
| 网络 | `launcher/net/Request.cpp`、`NetJob.cpp`、`HttpMetaCache.cpp`、`MetaCacheSink.cpp`、`FileSink.cpp` |
| Java | `launcher/java/*`、`launcher/minecraft/launch/AutoInstallJava.cpp`、`launcher/launch/steps/CheckJava.cpp` |
| 任务 | `launcher/tasks/Task.*`、`ConcurrentTask.*`、`SequentialTask.*` |
| 加载器 | `launcher/ui/dialogs/InstallLoaderDialog.cpp`、`launcher/minecraft/Component.cpp`、meta 抽样 |
| 整合包 | `launcher/InstanceImportTask.cpp`、`modplatform/modrinth/*`、`modplatform/flame/*`、`modplatform/technic/*` |
| 账户 | `launcher/minecraft/auth/*`（`AuthFlow`、`steps/*`、`MinecraftAccount`、`AccountData`） |
| 配置 | `launcher/settings/*`（`INISettingsObject`、`OverrideSetting`）、`launcher/BaseInstance.cpp` |

---

## 11. 待验证 / 不确定项

1. **meta 服务端转换规则**不在本仓库：`arguments→minecraftArguments`、`logging` 的保留、Forge 包的生成逻辑都在 meta 仓库；本报告基于抽样数据推断，未读服务端代码。
2. **运行时内存/启动耗时**未实测（未构建）；8.2 是结构判断。
3. **OptiFine** 确认无专门安装器；但用户可通过 jar mod / Forge mod 使用，未验证所有版本。
4. **ForgeWrapper 的具体行为**（何时跑 processors、缓存位置）在第三方 jar 内，本仓库只包含调用入口（主类名）与下载清单。
5. **Quilt/Fabric 的完整库清单**随版本变化，报告中只引用抽样版本。
