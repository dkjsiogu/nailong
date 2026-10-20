# 04 · 版本模型与磁盘布局

> 本文回答四个问题：版本 JSON 长什么样、如何沿 `inheritsFrom` 合并、实例如何引用版本、文件放在哪里。
> 2026 年新格式（`26.x`、`default-user-jvm`、natives 子目录）的实测记录见 [附录 A](appendix-a-version-json-2026.md)。

## 1. 术语

| 术语 | 定义 | 落盘位置 |
|---|---|---|
| **Manifest / Version JSON** | Mojang 或加载器提供的版本描述 | `shared/versions/<id>/<id>.json` |
| **Profile** | 一个可被 `inheritsFrom` 引用的 manifest；可以继承另一个 profile | 同上 |
| **Base version** | 原版（vanilla）版本，如 `1.20.1`、`26.3` | `shared/versions/1.20.1/1.20.1.json` |
| **Loader profile** | Fabric/Quilt/Forge/NeoForge 生成的 profile，通常 `inheritsFrom` base | `shared/versions/<loader>-.../<id>.json` |
| **Instance** | 用户可见的配置单元；引用一个 profile + 游戏目录 + Java/内存等 | `instances/<slug>/instance.toml` |
| **Resolve** | 沿继承链合并出扁平 manifest 的纯函数 | 缓存于 `instances/<slug>/.cache/` |
| **Component** | 构成实例的可安装单元：原版 + 一个加载器（v1 限制为单加载器） | 记录在 `instance.toml` 的 `[version]` |

## 2. 磁盘布局

```text
<root>/                                  # 默认 ~/.nailong（Windows: %APPDATA%\nailong）
├─ config.toml                           # 全局设置（TOML，原子写）
├─ accounts.json                         # 账户公开元数据；密钥在系统钥匙串
├─ instances/
│  └─ <slug>/                            # slug 由显示名清洗而来，唯一
│     ├─ instance.toml                   # 实例配置（唯一事实来源）
│     ├─ minecraft/                      # 隔离时的游戏目录（mods/saves/options.txt）
│     ├─ natives/<key>/                  # 解压后的 natives（按 profile+平台缓存）
│     ├─ logs/                           # 该实例最近一次启动日志
│     └─ .cache/resolved-<hash>.json     # 解析结果缓存（可删除）
├─ shared/                               # 可指向已有 .minecraft（高级设置）
│  ├─ versions/<id>/<id>.json
│  ├─ versions/<id>/<id>.jar             # 原版 client jar；loader profile 通常无 jar
│  ├─ libraries/<maven 布局>/...
│  ├─ assets/
│  │  ├─ indexes/<asset-id>.json
│  │  ├─ objects/<xx>/<sha1>
│  │  ├─ virtual/<asset-id>/...          # ≤1.6 的虚拟资源（硬链接/复制）
│  │  └─ log_configs/<log-id>            # log4j 配置
│  └─ jre/<component>/<platform>/...     # 托管 Java 运行时
├─ cache/
│  ├─ meta/<key>.json                    # 元数据 + ETag/Last-Modified
│  ├─ downloads/                         # .part 与临时下载
│  └─ icons/                             # 头像/图标缓存
└─ logs/nailong.log                      # 启动器日志（轮转）
```

### 2.1 为什么 libraries/assets 共享，游戏目录隔离

- libraries 与 assets 是**只读、内容寻址/按坐标存储**的，跨实例共享可节省数 GB。
- 游戏目录包含 `mods/`、`config/`、`saves/`，跨版本共享会冲突；默认隔离（见 §7）。
- `shared/` 可配置为已有 `.minecraft` 目录，从而直接复用 PCL/HMCL/官方启动器已下载的资源（无需重复下载）。

### 2.2 命名与清洗

- 实例目录名 `slug`：显示名做 Unicode 保留、去掉 Windows 非法字符 `<>:"/\|?*`、控制字符、结尾空格/点；截断到 64 字符；重名追加 `-2`、`-3`。
- 允许中文目录名；但启动时检查路径是否含非 ASCII，旧版 LWJGL 需要特殊处理（见 [05](05-launch-pipeline.md#6-natives)）。
- Profile id 使用规范 id（如 `1.20.1-forge-47.4.10`），只允许 `[A-Za-z0-9._+-]`；其他字符替换为 `_`。Forge/Fabric 的 id 本身满足。

## 3. Version JSON 模型

### 3.1 已知字段

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,              // release / snapshot / old_beta / old_alpha
    #[serde(rename = "mainClass", default)]
    pub main_class: Option<String>,
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub jar: Option<String>,               // 主 jar 重定向到另一个版本 id
    #[serde(default)]
    pub assets: Option<String>,            // ≤1.7 的 assets 目录名
    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(rename = "minecraftArguments", default)]
    pub legacy_arguments: Option<String>,  // ≤1.12 的整串参数
    #[serde(default)]
    pub arguments: Option<Arguments>,      // 1.13+ 的结构化参数
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub downloads: Downloads,              // client/server/client_mappings
    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub logging: Option<Logging>,
    #[serde(rename = "minimumLauncherVersion", default)]
    pub minimum_launcher_version: Option<u32>,
    #[serde(rename = "complianceLevel", default)]
    pub compliance_level: Option<u32>,
    #[serde(rename = "releaseTime", default)]
    pub release_time: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    /// 未知字段原样保留：第三方加载器/未来 Mojang 字段不丢。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

### 3.2 参数

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
    /// 2026 年新增：Mojang 建议的默认用户 JVM 参数（含内存/GC）。
    #[serde(rename = "default-user-jvm", default)]
    pub default_user_jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Ruled { rules: Vec<Rule>, value: StringOrList },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrList { One(String), Many(Vec<String>) }
```

- `Plain` 字符串可能包含空格（Fabric 的 `-DFabricMcEmu= net.minecraft.client.main.Main `）——**整体作为一个 argv**，绝不按空格切分。
- `legacy_arguments`（`minecraftArguments`）是按空格分词的字符串；仅对旧版本使用。

### 3.3 库

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub name: String,                       // group:artifact:version[:classifier][@ext]
    #[serde(default)]
    pub downloads: Option<LibDownloads>,    // artifact + classifiers
    #[serde(default)]
    pub natives: Option<BTreeMap<String, String>>, // 旧格式：os -> classifier（含 ${arch}）
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub extract: Option<Extract>,
    #[serde(default)]
    pub url: Option<String>,                // 无 downloads 时的 maven 基地址
    #[serde(default)]
    pub checksums: Vec<String>,             // 部分 Forge 库内嵌校验
}

pub struct LibDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<BTreeMap<String, Artifact>>,
}

pub struct Artifact {
    pub path: String,                       // maven 相对路径
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub url: String,
}

pub struct Extract { #[serde(default)] pub exclude: Vec<String> }
```

库坐标解析规则：

| 形式 | 含义 |
|---|---|
| `group:artifact:version` | 普通库 |
| `group:artifact:version:classifier` | 带分类器（natives、sources 等） |
| `group:artifact:version@zip` | 改变扩展名 |
| `natives` 映射 | 旧版：`natives-windows-${arch}` → 替换 `${arch}` 为 `32`/`64` |
| `name` 中的 `natives-*` | 新版：natives 直接作为带分类器的 artifact 下载（见 26.3 的 LWJGL 3.4） |

**native 判定**：`natives` 非空，或坐标的 classifier 以 `natives-` 开头。native 库**不进 classpath**，只解压。

### 3.4 规则

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: RuleAction,                 // allow | disallow
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<BTreeMap<String, bool>>,
}

pub struct OsRule {
    pub name: Option<String>,               // windows | osx | linux
    pub arch: Option<String>,               // x86 | x86_64 | arm64
    pub version: Option<String>,            // 旧式精确匹配（少见）
    #[serde(rename = "versionRange")]
    pub version_range: Option<VersionRange>, // 2026 新式：min/max，如 Windows 10.0.17134
}
```

判定算法（与 Mojang 规范一致）：

```text
allowed = true
for rule in rules:
    if rule.matches(os, arch, features, os_version):
        allowed = (rule.action == Allow)
return allowed
```

- 无 `rules` → 允许。
- `features`：`is_demo_user`、`has_custom_resolution`、`has_quick_plays_support`、`is_quick_play_singleplayer/multiplayer/realms`。未提供的 feature 视为 `false`。
- 架构归一化：`amd64`/`x86_64` → `x86_64`；`aarch64`/`arm64` → `arm64`；`x86`/`i386` → `x86`。
- `versionRange`：取系统版本（Windows 用 `RtlGetVersion`/注册表，Linux/macOS 用 `uname`），按点分数字比较；无法获取时**保守返回 false**（例如不启用需要新 Windows 的 ZGC 参数）。

### 3.5 资源与日志

```rust
pub struct AssetIndexRef { pub id: String, pub sha1: Option<String>, pub size: Option<u64>, pub total_size: Option<u64>, pub url: Option<String> }
pub struct Logging { pub client: Option<LoggingClient> }
pub struct LoggingClient { pub argument: String, pub file: Artifact, #[serde(rename = "type")] pub kind: String }
```

- 1.7+：`assetIndex` 指向对象存储索引；下载到 `assets/indexes/<id>.json`，对象到 `assets/objects/<xx>/<sha1>`。
- ≤1.6：`assets` 是目录名（如 `legacy`），需要把对象重建为 `assets/virtual/<id>/` 的扁平目录树。
- `logging.client.file` 下载到 `assets/log_configs/<file.id>`，启动参数中的 `${path}` 替换为该绝对路径。

## 4. 继承与合并

### 4.1 解析流程

```text
resolve(id, visiting=[]):
  if id in visiting: error(CircularInheritance)
  manifest = load_profile(id)                  # 读 shared/versions/<id>/<id>.json
  if manifest.inherits_from is None:
      return manifest
  parent = resolve(manifest.inherits_from, visiting + [id])
  return merge(parent, manifest)               # 子优先（见下）
```

- 最大深度 32；超过报错（正常最多 2–3 层）。
- `inherits_from` 指向不存在的 profile：返回 `Error::VersionNotFound`，由安装器尝试修复。
- 解析结果缓存 key = `sha1(所有链上 profile 文件字节 + OS + ARCH + 版本) `；profile 变化自动失效。

### 4.2 合并规则

| 字段 | 规则 |
|---|---|
| `id` | 子（叶）的 id |
| `type` / `releaseTime` / `time` | 子有则用子，否则父 |
| `mainClass` | 子有则用子 |
| `inheritsFrom` | 置空（已展开） |
| `assets` / `assetIndex` | 子有则用子 |
| `javaVersion` | 子有则用子 |
| `downloads` | 按 key 合并；子优先（client jar 通常来自原版） |
| `logging` | 子有则用子 |
| `libraries` | **子在前，父在后**；按 `(group, artifact, classifier, ext)` 去重，首次出现优先 |
| `arguments.game` | **父在前，子在后**（拼接） |
| `arguments.jvm` | **父在前，子在后**（拼接） |
| `arguments.default-user-jvm` | 子有则用子，否则父（不拼接） |
| `legacyArguments` | 子有则用子 |
| `jar` | 子有则用子 |
| `minimumLauncherVersion` | 取 `max` |
| `extra` | 浅合并，子优先 |

> **为什么 libraries 子前父后，而 arguments 父前子后？**
> 加载器需要"覆盖"原版库（例如更高版本的 ASM），因此子库优先；而 JVM/游戏参数的语义是"追加"（Forge 在主类前追加 `--launchTarget`），父参数先出现更符合直觉。HMCL 采用同样的非对称规则（见 [01](01-prior-art.md#41-关键事实)），实测兼容性最好。

### 4.3 去重细节

- 坐标解析后 key 为 `(group, artifact, version_ignored, classifier, extension)`；**忽略 version**，否则无法用子版本覆盖父版本。
- 保留首次出现的元素（子优先），但保留其**位置**，以维持 classpath 顺序。
- `downloads.classifiers` 与 `natives` 中的 native 候选单独合并；同名 native 只保留一个。
- 同一 group:artifact 的不同 classifier（如 `natives-windows` 与 `natives-linux`）视为不同 key，只保留当前平台匹配的那个。

## 5. 加载器 profile

### 5.1 统一形态

所有加载器安装器最终都产出一个 **标准 version JSON**（含 `inheritsFrom`）写入 `shared/versions/<id>/<id>.json`，并下载所需库。这样：

- 启动器只需实现"解析 + 启动"，不需要在启动路径里理解加载器差异。
- 与官方启动器、Prism、MultiMC 的 `versions/` 布局兼容。
- 加载器升级 = 新建一个 profile；实例切换引用即可回滚。

```json
{
  "id": "fabric-loader-0.18.1-1.21.11",
  "inheritsFrom": "1.21.11",
  "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
  "arguments": { "game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "] },
  "libraries": [ { "name": "net.fabricmc:fabric-loader:0.18.1", "url": "https://maven.fabricmc.net/" } ]
}
```

### 5.2 各加载器的 id 与来源

| 加载器 | Profile id | 来源 | 是否需要 processors |
|---|---|---|---|
| Vanilla | `<mc>` | Mojang version manifest | 否 |
| Fabric | `fabric-loader-<loader>-<mc>`（Fabric meta 自带 id） | `https://meta.fabricmc.net/v2/versions/loader/<mc>/<loader>/profile/json` | 否 |
| Quilt | `quilt-loader-<loader>-<mc>` | `https://meta.quiltmc.org/v3/versions/loader/<mc>/<loader>/profile/json` | 否 |
| Forge | installer `version.json` 的 `id`（如 `1.20.1-forge-47.4.10`） | Forge installer 内 | 是（1.13+） |
| NeoForge | installer `version.json` 的 `id` | NeoForge installer 内 | 是 |

### 5.3 互操作与扩展字段

- **HMCL** 的 root 实例 JSON 含 `patches`、`root`；我们**读取时忽略**这些字段（保留在 `extra`），只用扁平字段。若要完整支持 HMCL 组件合成，未来可作为导入适配器，而非核心模型。
- **Prism/MultiMC** 用 `mmc-pack.json` 描述组件；导入时映射为"base + loader"并生成标准 profile（见 [06](06-install-download.md#8-整合包导入)）。
- **Forge** 的库可能只有 `name` + 内嵌 `maven/`（installer 自带），安装器负责复制到 `shared/libraries`。
- 未知字段一律保留；写回时不丢失。

## 6. 实例配置

### 6.1 `instance.toml`

```toml
schema_version = 1
id = "my-fabric-world"              # 与目录名一致
name = "My Fabric World"
created_at = "2026-10-20T10:00:00Z"
last_played_at = "2026-10-20T12:30:00Z"
play_time_secs = 5400
icon = "icon.png"                   # 相对实例目录；缺省用版本图标

[version]
profile = "fabric-loader-0.18.1-1.21.11"
base = "1.21.11"
loader = "fabric"                   # vanilla | fabric | quilt | forge | neoforge
loader_version = "0.18.1"           # vanilla 为空

[java]
mode = "auto"                       # auto | managed | custom
path = ""                           # custom 时必填
major = 21                          # 0 = 自动推断

[launch]
memory_mb = 4096
min_memory_mb = 1024
override_jvm_args = false           # true 时忽略启动器自动生成的 JVM 参数
jvm_args = []                       # 数组，避免 shell 解析歧义
game_args = []
width = 1280
height = 720
fullscreen = false
server = ""                         # 可选：启动后直连
demo = false
env = {}                            # 额外环境变量
wrapper = []                        # 可选：如 gamescope/mangohud

[game]
isolation = "auto"                  # auto | always | never
game_dir = ""                       # 自定义绝对路径，空则按 isolation 解析
pack_source = ""                    # modrinth / curseforge / mmc / manual
pack_id = ""
pack_version = ""
```

### 6.2 字段语义

- `version.profile` 是**唯一事实来源**；`base`/`loader`/`loader_version` 用于 UI 展示与修复（profile 丢失时可重建）。
- `java.mode = "auto"`：按 manifest 的 `javaVersion` 与规则表选择；`managed` 强制使用托管 JRE；`custom` 使用 `path`。
- `launch.jvm_args` / `game_args` 是**用户追加参数**，不是完整覆盖；高级设置提供"完全覆盖"开关（`override_jvm_args`，默认 false）。
- `game.isolation = "auto"`：有加载器或整合包 → 实例目录；纯原版 → 共享目录。
- 所有时间使用 UTC RFC 3339；UI 本地化显示。

### 6.3 配置迁移

- `schema_version` 逐级迁移；迁移前备份为 `instance.toml.bak-<version>`。
- 未知字段保留（`#[serde(flatten)] extra`），但 UI 不展示。
- 读取失败：记录错误并把实例标记为"损坏"，不阻塞其他实例加载。

### 6.4 全局配置（`config.toml`）

```toml
schema_version = 1

[general]
language = "zh-CN"            # zh-CN | en-US | auto
theme = "auto"                # auto | light | dark
check_updates = true
close_action = "quit"         # quit | minimize（v1.1 托盘）

[storage]
# 共享 libraries/assets/versions 根；空 = <root>/shared
# 可指向已有 .minecraft 以复用资源
shared_dir = ""
portable = false

[download]
source = "auto"               # auto | official | mirror | custom
custom_mirror = ""
concurrency = 32              # 全局并发
per_host = 8
proxy = ""                    # http://... | socks5://... | 空 = 系统代理
timeout_secs = 30

[java]
auto_download = true
extra_paths = []              # 手动添加的 JDK 路径

[launch]
memory_mb = 0                 # 0 = 自动（按系统内存 + manifest 默认）
default_jvm_args = []
default_game_args = []
keep_launcher_open = false

[ui]
font_scale = 1.0
show_snapshots = false
```

- 所有字段有默认值；缺失/旧版本可迁移。
- `memory_mb = 0` 表示按 §8.4 的规则自动选择。
- 未知字段保留（`#[serde(flatten)] extra`），便于未来版本回退。

## 7. 运行目录（版本隔离）

```rust
pub enum Isolation { Auto, Always, Never }

pub fn resolve_run_dir(root: &Root, inst: &Instance) -> PathBuf {
    match (inst.game.isolation, inst.game.game_dir.is_empty()) {
        (Isolation::Always, true) => root.instance_dir(&inst.id).join("minecraft"),
        (Isolation::Never,  true) => root.shared_dir().join("minecraft"),
        (_, false)                => PathBuf::from(&inst.game.game_dir),
        (Isolation::Auto, true)   => {
            if inst.version.loader != Loader::Vanilla || !inst.game.pack_source.is_empty() {
                root.instance_dir(&inst.id).join("minecraft")
            } else {
                root.shared_dir().join("minecraft")
            }
        }
    }
}
```

- **不是** `is_isolated: bool`：用户可以从"隔离"切到"共享"而不丢文件；解析函数是纯函数，可单测。
- 共享目录是 `shared/minecraft`；若 `shared` 指向已有 `.minecraft`，则共享目录就是那个 `.minecraft`。
- 整合包导入的实例强制 `Always`（避免污染共享目录）。
- 切换隔离模式时**不自动搬移** `mods/saves`；UI 提示"文件仍在新目录，可手动复制"，并提供"复制过去"按钮（v1.1）。

## 8. 解析缓存

```text
instances/<slug>/.cache/resolved-<key>.json
```

包含：

```json
{
  "key": "sha1:...",
  "profile": "fabric-loader-0.18.1-1.21.11",
  "chain": ["1.21.11", "fabric-loader-0.18.1-1.21.11"],
  "resolved": { "...": "扁平 manifest，字段同 VersionJson" }
}
```

- 缓存只是性能优化；删除后可从 profile 重建。
- 启动前校验 key（profile 文件 SHA1 + OS/ARCH）；不匹配则重新解析。
- 不缓存"是否已下载"的状态——磁盘才是事实。

## 9. 边界情况清单

| 情况 | 处理 |
|---|---|
| `inheritsFrom` 成环 | 报 `CIRCULAR_INHERITANCE`，附链 |
| 父 profile 缺失 | 报 `VERSION_NOT_FOUND`；若实例有 loader 元数据，尝试重装 profile |
| `downloads.client` 缺失 | 沿链查找；仍无则报 `CLIENT_JAR_MISSING`（旧版/自定义 profile） |
| `jar` 指向其他版本 | 使用该版本目录下的 jar，不回退到 id 同名 jar |
| 同一坐标不同版本 | 子优先、保留首次出现位置 |
| 现代 natives 无 `natives` 字段 | 用坐标 classifier `natives-*` 判定 |
| 旧版 `natives` 含 `${arch}` | 替换为 `32`/`64`（按进程架构） |
| asset index 文件被 Mojang 修改 | 以 `sha1` 校验；不匹配则重新下载 |
| 版本 id 含 `/` 或空格 | 写入前清洗；报告原 id 供展示 |
| `minimumLauncherVersion` 高于我们 | 仅警告，不阻止（Mojang 用其做兼容提示） |
| 26.x 的 `default-user-jvm` | 按 §3.2 解析；内存设置优先（见 05 §8） |
| 26.x 的 `-Djava.library.path=${natives_directory}/java` | 动态推导实际 natives 目录（见 05 §6） |
| 中文/空格路径 | 原样传递（argv 不经过 shell）；旧版 LWJGL 用 ASCII 临时目录规避 |
| Windows 长路径 (>260) | 启动前检测并提示；内部尽量使用 `\\?\` 前缀，但传给 Java 的路径保持普通形式 |

## 10. 测试夹具

固定保留以下真实 JSON（裁剪到必要字段，保留未知字段示例）：

| 夹具 | 覆盖点 |
|---|---|
| `1.7.10.json` | `minecraftArguments`、旧 assets、`extract`、无 `arguments` |
| `1.12.2.json` | 旧版 Forge 基线、`launchwrapper`、Java 8 规则 |
| `1.20.1.json` | 现代 `arguments`、natives classifier、logging |
| `1.21.11.json` | Java 21、资产索引、规则 |
| `26.3.json` | 日期式版本、Java 25、`default-user-jvm`、natives 子目录、`os.versionRange` |
| `fabric-loader-*.json` | 无 `downloads`、带 `url` + hash 的库、带空格的 jvm arg |
| `forge-1.20.1-install-profile.json` | processors、data 占位符、sides |
| `neoforge-*.json` | 不同 maven/主类 |
| `mmc-pack.json` | 导入映射 |

快照测试断言：合并后的 `mainClass`、classpath 顺序、参数列表、native 列表、`javaVersion`。任何变更必须显式更新快照并在 PR 中说明原因。
