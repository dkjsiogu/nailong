# 11 · M0 实施契约（可开工版）

> 目的：把 [03 架构](03-architecture.md) / [04 版本模型](04-version-model.md) / [05 启动流水线](05-launch-pipeline.md) 收敛成**可直接编码的接口、顺序与验收**。
> 范围：M0 只做"解析 + 启动已安装的版本"，**不做下载、不做 GUI、不做微软登录**。目标是两周内能用 CLI 启动 1.20.1 与 26.3。

## 1. M0 边界

### 做

- Cargo workspace、CI、错误/进度/路径/配置骨架。
- 读取已有 `.minecraft` 布局：`versions/<id>/<id>.json`、`libraries/`、`assets/`。
- 版本 JSON 解析、`inheritsFrom` 合并、规则、classpath、参数、natives。
- Java 探测/选择（系统已安装 Java）。
- `LaunchPlan` 生成、进程启动、日志转发、取消。
- CLI：`list` / `resolve` / `java list` / `launch --print-plan` / `launch`。
- 假 Java 测试与真实启动冒烟。

### 不做（明确留给 M1+）

- 版本清单/下载/安装/校验/修复。
- Java 运行时下载。
- 实例目录/`instance.toml`/GUI（M0 用 CLI 参数直接指定 profile 与游戏目录）。
- 微软登录/authlib-injector（离线启动令牌用占位）。
- 加载器安装（M0 可启动**已经安装好**的 Fabric/Forge profile，用于验证合并逻辑）。

## 2. Workspace 与文件

```text
nailong/
├─ Cargo.toml
├─ rust-toolchain.toml            # 1.95
├─ rustfmt.toml
├─ clippy.toml
├─ .cargo/config.toml             # Windows crt-static（可选）
├─ crates/
│  ├─ nailong-core/
│  │  ├─ Cargo.toml
│  │  ├─ src/
│  │  │  ├─ lib.rs
│  │  │  ├─ error.rs
│  │  │  ├─ progress.rs
│  │  │  ├─ paths.rs
│  │  │  ├─ config.rs
│  │  │  ├─ model/
│  │  │  │  ├─ mod.rs
│  │  │  │  ├─ version.rs        # VersionJson/Library/Rule/Arguments
│  │  │  │  ├─ instance.rs       # M0 只读视图
│  │  │  │  └─ java.rs           # JavaRuntime/JavaRequirement
│  │  │  ├─ resolve/
│  │  │  │  ├─ mod.rs            # resolve() + merge()
│  │  │  │  ├─ rules.rs
│  │  │  │  ├─ classpath.rs
│  │  │  │  └─ args.rs
│  │  │  ├─ launch/
│  │  │  │  ├─ mod.rs
│  │  │  │  ├─ java.rs           # detect/select/probe
│  │  │  │  ├─ plan.rs
│  │  │  │  ├─ natives.rs
│  │  │  │  ├─ exec.rs
│  │  │  │  └─ log.rs
│  │  │  └─ util/
│  │  │     ├─ fs.rs
│  │  │     ├─ hash.rs
│  │  │     └─ os.rs
│  │  └─ tests/
│  │     ├─ fixtures/
│  │     │  ├─ versions/
│  │     │  ├─ java/
│  │     │  └─ natives/
│  │     ├─ resolve_golden.rs
│  │     ├─ rules.rs
│  │     ├─ args_golden.rs
│  │     ├─ java_select.rs
│  │     └─ exec_fake_java.rs
│  └─ nailong-cli/
│     ├─ Cargo.toml
│     └─ src/main.rs
├─ docs/
└─ scripts/
   ├─ fake-java.sh
   └─ fake-java.bat
```

依赖方向：`nailong-cli → nailong-core`；core 内 `model ← resolve ← launch`，`util` 无业务依赖。M0 的 core 依赖只需：`serde`、`serde_json`、`thiserror`、`tracing`、`sha1`、`zip`（natives 解压）、`tokio`（进程/日志）、`directories`（可选）、`uuid`（离线 UUID）。**不引入 reqwest/egui/keyring**。

## 3. 核心类型（M0 冻结）

### 3.1 ID 与路径

```rust
pub struct ProfileId(pub String);      // 版本目录名，也是 JSON 的 id
pub struct InstanceId(pub String);     // M0 可为空
pub struct TaskId(pub u64);

pub struct Root { root: PathBuf }      // 指向已有 .minecraft
impl Root {
    pub fn new(root: impl Into<PathBuf>) -> Self;
    pub fn root(&self) -> &Path;
    pub fn versions(&self) -> PathBuf;      // root/versions
    pub fn libraries(&self) -> PathBuf;     // root/libraries
    pub fn assets(&self) -> PathBuf;        // root/assets
    pub fn version_dir(&self, id: &ProfileId) -> PathBuf;
    pub fn version_json(&self, id: &ProfileId) -> PathBuf;
    pub fn client_jar(&self, id: &ProfileId) -> PathBuf;  // 解析 jar 字段后再定
}
```

路径规则：

- 所有路径先 `to_absolute`（基于 `root`），不做 `canonicalize`（避免 Windows UNC/长路径副作用）；**写操作**才做前缀校验。
- 不允许从 JSON 拼接出 `..`/绝对路径；库路径只允许 maven 相对路径。
- Windows 路径原样作为 argv；不经过 shell。

### 3.2 规则上下文

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os { Windows, MacOs, Linux }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch { X86, X86_64, Arm64 }

#[derive(Debug, Clone)]
pub struct ResolveContext {
    pub os: Os,
    pub arch: Arch,
    pub os_version: Option<String>,   // Windows: "10.0.22631"
    pub features: Features,
}

#[derive(Debug, Clone, Default)]
pub struct Features {
    pub demo: bool,
    pub custom_resolution: bool,
    pub quick_play: bool,             // has_quick_plays_support
    pub quick_play_singleplayer: bool,
    pub quick_play_multiplayer: bool,
    pub quick_play_realms: bool,
}

impl ResolveContext {
    pub fn current() -> Self;         // 读 OS/arch/版本；features 默认 false
}
```

规则求值必须与 [04 §3.4](04-version-model.md#34-规则) 一致：**无 rules 允许；有 rules 初始 false；按顺序最后匹配者生效；规则内所有谓词 AND**。

### 3.3 版本 JSON（M0 必需字段）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "type", default)] pub kind: Option<String>,
    #[serde(rename = "mainClass", default)] pub main_class: Option<String>,
    #[serde(rename = "inheritsFrom", default)] pub inherits_from: Option<String>,
    #[serde(default)] pub jar: Option<String>,
    #[serde(default)] pub assets: Option<String>,
    #[serde(rename = "assetIndex", default)] pub asset_index: Option<AssetIndexRef>,
    #[serde(rename = "minecraftArguments", default)] pub legacy_arguments: Option<String>,
    #[serde(default)] pub arguments: Option<Arguments>,
    #[serde(default)] pub libraries: Vec<Library>,
    #[serde(default)] pub downloads: Downloads,
    #[serde(rename = "javaVersion", default)] pub java_version: Option<JavaVersion>,
    #[serde(default)] pub logging: Option<Logging>,
    #[serde(rename = "minimumLauncherVersion", default)] pub minimum_launcher_version: Option<u32>,
    #[serde(rename = "releaseTime", default)] pub release_time: Option<String>,
    #[serde(default)] pub time: Option<String>,
    #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
}
```

`Library`、`Rule`、`Arguments`、`Artifact`、`Downloads`、`Logging` 的定义见 [04 §3](04-version-model.md#3-version-json-模型)，M0 必须支持：

- `downloads.artifact` / `downloads.classifiers`；
- `natives` 旧映射 + `natives-*` classifier；
- `rules` 的 os.name/arch/version/versionRange/features；
- `extract.exclude`；
- `name` 的 `group:artifact:version[:classifier][@ext]`。

### 3.4 合并与解析

```rust
pub trait ProfileLoader {
    fn load(&self, id: &ProfileId) -> Result<VersionJson>;
}

pub struct FsProfileLoader { root: Root }
impl ProfileLoader for FsProfileLoader { /* 读 versions/<id>/<id>.json */ }

pub struct ResolveRequest<'a> {
    pub profile: ProfileId,
    pub ctx: ResolveContext,
    pub loader: &'a dyn ProfileLoader,
}

pub fn resolve(req: ResolveRequest<'_>) -> Result<ResolvedVersion>;

#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    pub id: String,
    pub chain: Vec<String>,                 // [vanilla, loader] 顺序
    pub main_class: String,
    pub assets: AssetIndexRef,
    pub libraries: Vec<Library>,            // 子前父后、已去重
    pub arguments: Arguments,
    pub legacy_arguments: Option<String>,
    pub downloads: Downloads,
    pub java_version: Option<JavaVersion>,
    pub logging: Option<LoggingClient>,
    pub client_jar_id: Option<String>,      // jar 重定向后的版本 id
    pub minimum_launcher_version: Option<u32>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

合并规则（必须用 golden test 锁定）：

1. 递归加载父 profile；环检测（`visiting` 集合）；深度 > 32 报错。
2. 标量：子有则用子。
3. `libraries`：**子在前、父在后**；按 `(group, artifact, classifier, ext)` 去重，保留首次出现。
4. `arguments.game/jvm`：**父在前、子在后**；`default-user-jvm` 子覆盖父。
5. `mainClass`/`assets`/`assetIndex`/`javaVersion`/`logging`/`jar`：子优先。
6. `downloads`：按 key 合并，子优先。
7. `minimumLauncherVersion`：取 `max`。
8. `id`：叶 id；`inheritsFrom` 清空；`extra` 浅合并。

> M0 不实现 HMCL `patches` 合成；读取时忽略 `patches`（保留在 `extra`）。Forge 的 `version.json` 是标准 profile，可直接解析。

### 3.5 Java

```rust
#[derive(Debug, Clone)]
pub struct JavaRuntime {
    pub path: PathBuf,          // bin/java 或 bin/java.exe
    pub home: PathBuf,
    pub major: u32,             // 8/16/17/21/25
    pub version: String,        // "21.0.7"
    pub vendor: Option<String>,
    pub arch: Arch,
    pub source: JavaSource,     // Managed | System | Custom | Cache
}

pub struct JavaRequirement {
    pub major: u32,
    pub min_patch: Option<u32>,
    pub max_major: Option<u32>,
    pub reasons: Vec<&'static str>,
}

pub trait JavaProbe {
    fn probe(&self, path: &Path) -> Result<JavaRuntime>;
}
pub struct FsJavaProbe;
pub struct FakeJavaProbe { pub outputs: HashMap<PathBuf, String> } // 测试用

pub fn detect_all(probe: &dyn JavaProbe) -> Vec<JavaRuntime>;
pub fn requirement_for(resolved: &ResolvedVersion) -> JavaRequirement;
pub fn select(candidates: &[JavaRuntime], req: &JavaRequirement) -> Result<JavaRuntime>;
```

M0 的探测顺序（详见 [06 §7.1](06-install-download.md#71-系统-java-探测)）：`JAVA_HOME` → `PATH` → 常见目录。探测方法：

- 优先读 `<home>/release` 的 `JAVA_VERSION`/`OS_ARCH`；
- 否则跑 `java -XshowSettings:properties -version`，解析 stderr；
- 超时 5 s；失败不加入候选。

### 3.6 启动计划

```rust
pub struct LaunchOptions {
    pub game_dir: PathBuf,             // M0 由 CLI 指定；默认 root
    pub width: u32,
    pub height: u32,
    pub demo: bool,
    pub quick_play: Option<QuickPlay>,
    pub extra_jvm_args: Vec<OsString>,
    pub extra_game_args: Vec<OsString>,
    pub memory_mb: u32,
    pub min_memory_mb: u32,
}

pub struct AuthSession {
    pub name: String,
    pub uuid: String,                  // 无连字符
    pub access_token: String,
    pub user_type: String,             // "msa"
    pub client_id: String,             // 可空
    pub xuid: String,                  // 可空
}

pub struct LaunchPlan {
    pub java: PathBuf,
    pub jvm_args: Vec<OsString>,
    pub main_class: String,
    pub game_args: Vec<OsString>,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub run_dir: PathBuf,
    pub client_jar: Option<PathBuf>,
    pub env: BTreeMap<OsString, OsString>,
    pub redactions: Vec<String>,
    pub log_path: PathBuf,
}

pub fn build_plan(
    root: &Root,
    resolved: &ResolvedVersion,
    auth: &AuthSession,
    java: &JavaRuntime,
    opts: &LaunchOptions,
) -> Result<LaunchPlan>;
```

M0 必须实现：

- classpath：规则过滤、native 排除、存在才加入、主 jar 最后、无 `-cp` 时兜底追加。
- natives：旧 `natives` 映射 + 新 classifier；解析 `-Djava.library.path=${natives_directory}/<sub>`；解压到 `<natives>/<sub>`；过滤 `META-INF`/`extract.exclude`；跳过符号链接。
- 变量表：`auth_*`、`version_name`、`version_type`、`game_directory`、`assets_root`、`assets_index_name`、`natives_directory`、`classpath`、`classpath_separator`、`library_directory`、`launcher_name/version`、`primary_jar(_name)`、`resolution_*`、`user_type`、`user_properties`、`clientid`、`xuid`。
- 参数顺序：见 [05 §8](05-launch-pipeline.md#8-参数装配顺序)。M0 自动参数只做：内存、`-Dfile.encoding=UTF-8`、`-Dlog4j2.formatMsgNoLookups=true`、launcher brand、`-Dminecraft.client.jar`；其余以 manifest 为准。
- `default-user-jvm`：M0 只解析并做 Java 兼容过滤/内存钳制；规则按 [05 §8.4](05-launch-pipeline.md#84-26x-的-default-user-jvm)。
- 26.3 的 natives 子目录必须正确。
- 日志：stdout/stderr 两路，写文件 + 环形缓冲；退出原因分类（M0 只需 Normal/JvmError/ApplicationError/Killed/Cancelled）。

### 3.7 执行

```rust
pub struct GameHandle {
    pub pid: u32,
    pub log_path: PathBuf,
    // M0: 只提供 wait/kill；日志由 exec 内部写文件
    pub async fn wait(self) -> Result<ExitStatus>;
    pub fn kill(&self) -> Result<()>;
}

pub async fn spawn(plan: &LaunchPlan, cancel: CancellationToken) -> Result<GameHandle>;
```

规则：

- `std::process::Command` / `tokio::process::Command` 直接 argv；`current_dir(run_dir)`；stdin null；stdout/stderr piped。
- Windows `CREATE_NO_WINDOW`。
- 默认移除 `_JAVA_OPTIONS` / `JAVA_TOOL_OPTIONS` / `JDK_JAVA_OPTIONS`。
- 取消：kill + wait，返回 `Error::Cancelled`。
- **不打印完整 argv**（含 token）；日志写脱敏摘要。

### 3.8 错误码（M0 最小集）

```rust
pub enum Error {
    Io { path: PathBuf, source: std::io::Error },
    Json { path: PathBuf, source: serde_json::Error },
    VersionNotFound { id: String },
    CircularInheritance { chain: Vec<String> },
    InheritanceTooDeep { depth: usize },
    MissingMainClass { id: String },
    MissingClientJar { id: String },
    MissingLibrary { path: PathBuf },
    UnsupportedLibraryName { name: String },
    JavaNotFound { required: u32 },
    JavaIncompatible { required: u32, found: u32 },
    JavaProbeFailed { path: PathBuf, reason: String },
    NativesExtract { path: PathBuf, source: zip::result::ZipError },
    SpawnFailed { java: PathBuf, source: std::io::Error },
    Cancelled,
}
```

`code()` 返回稳定字符串（`FS_IO` / `JSON_PARSE` / `VERSION_NOT_FOUND` / `INHERIT_CYCLE` / `INHERIT_DEPTH` / `MAIN_CLASS_MISSING` / `CLIENT_JAR_MISSING` / `LIBRARY_MISSING` / `LIBRARY_NAME_INVALID` / `JAVA_NOT_FOUND` / `JAVA_INCOMPATIBLE` / `JAVA_PROBE_FAILED` / `NATIVES_EXTRACT` / `SPAWN_FAILED` / `CANCELLED`）。UI 只映射 code，不解析 message。

## 4. CLI 契约

```bash
# 查看已有版本
nailong --root ~/.minecraft list

# 解析并打印计划（不启动）
nailong --root ~/.minecraft resolve 1.20.1 --print-plan
nailong --root ~/.minecraft resolve fabric-loader-0.18.1-1.21.11 --print-plan

# 查看 Java
nailong java list
nailong --root ~/.minecraft launch 1.20.1 --java /usr/lib/jvm/java-17/bin/java --dry-run

# 启动（M0 用占位离线账户；M1 接账户系统）
nailong --root ~/.minecraft launch 1.20.1 \
  --game-dir ~/.minecraft \
  --username Steve --memory 4096
```

约束：

- `--root` 默认 `$PWD/.minecraft`？M0 为显式参数；无参数时报错并提示。
- `--print-plan` 输出 JSON（便于快照测试）或人类可读；两者由 `--json` 切换。
- `--dry-run` 只解压 natives、不 spawn。
- 退出码：0 成功；1 用户错误；2 内部错误；3 取消。
- 所有路径用 `PathBuf`，不拼 shell 字符串。

## 5. 测试与夹具

### 5.1 夹具

```text
tests/fixtures/versions/
  1.7.10.json
  1.12.2.json
  1.20.1.json
  1.21.11.json
  26.3.json
  fabric-loader-0.18.1-1.21.11.json
  1.20.1-forge-47.4.10.json       # 从 installer 的 version.json 提取
tests/fixtures/java/
  release-8, release-17, release-21, release-25
tests/fixtures/natives/
  natives-win.zip                 # 含 .dll + META-INF + extract.exclude 样例
  natives-linux.zip               # 含 .so + 可执行位
  natives-traversal.zip           # 含 ../evil（必须拒绝）
```

- 真实 JSON 裁剪到 M0 需要的字段，但保留 `extra` 未知字段各一个，测试不丢字段。
- 夹具来源与抓取日期写在 `tests/fixtures/README.md`；不包含游戏 jar/资源。

### 5.2 Golden 测试

| 测试 | 断言 |
|---|---|
| `resolve 1.20.1` | mainClass、library 数、client jar、logging、java 17 |
| `resolve fabric 1.21.11` | mainClass Knot、Fabric 库在前、`-DFabricMcEmu= net.minecraft.client.main.Main ` 单参数 |
| `resolve forge 1.20.1` | mainClass BootstrapLauncher、`-DlibraryDirectory`、`-p`、继承原版 `-cp` |
| `resolve 26.3` | `default-user-jvm`、natives `/java`、Java 25、`--enable-native-access` |
| `resolve 1.7.10` | `minecraftArguments`、旧 assets、无 `arguments` |
| rules | windows/linux/mac、x86/arm64、features、versionRange |
| classpath | 顺序、去重、native 排除、缺失文件行为 |
| args | 变量替换、空格参数、未知变量保留 |
| java | 8/17/21/25 选择；LaunchWrapper ≤8；不兼容错误 |
| exec | 假 Java 记录 argv；取消杀进程；脱敏 |

快照规范化：把绝对路径替换为 `<ROOT>`、token 替换为 `<TOKEN>`、时间替换为 `<TIME>`，避免跨机器不稳定。

### 5.3 真实冒烟（人工/夜间）

- 用真实 `.minecraft` 启动 1.20.1（Java 17）到主菜单。
- 启动 26.3（Java 25）到主菜单，验证 natives 子目录。
- 启动一个已安装的 Fabric 1.21.11 profile。
- 启动 1.12.2 Forge（若已有安装），验证 `--tweakClass`。

## 6. 实现顺序（建议按天）

| 天 | 任务 | 产出 |
|---|---|---|
| 1 | workspace、CI、`error/progress/paths/config` | `cargo test` 绿 |
| 2 | `model` + 5 个版本 JSON 解析 | 解析测试 |
| 3 | `rules` + `merge/resolve` | golden 快照 |
| 4 | `classpath` + `args` | 计划快照 |
| 5 | Java 探测/选择 | 假 Java 测试 |
| 6 | natives 解压 | 夹具测试 |
| 7 | `exec` + CLI `list/resolve` | `--print-plan` |
| 8 | 真实启动 1.20.1 | 主菜单 |
| 9 | 26.3 + Fabric/Forge 解析 | 快照 |
| 10 | 取消/日志/错误码/复盘 | M0 验收 |

## 7. M0 验收清单

- [ ] `cargo fmt --check` / `clippy -D warnings` / `test` 全绿。
- [ ] 5 个版本 JSON 解析与合并快照通过。
- [ ] 1.20.1 与 26.3 真实启动成功（系统 Java）。
- [ ] Fabric/Forge profile 能解析出正确 mainClass/classpath/参数（不要求 M0 安装）。
- [ ] 中文路径/实例名启动成功，或明确记录限制。
- [ ] 取消启动后无残留 Java 进程。
- [ ] 日志/错误不含 access token。
- [ ] `--print-plan` 可用于与 HMCL/Prism 对比。
- [ ] 代码中无 `reqwest`/`egui`/`keyring` 依赖。

## 8. 待确认（M0 内解决）

1. Windows 上 `os_version` 的获取方式：注册表 `RtlGetVersion` vs `os_info`；失败时规则如何保守。
2. 26.3 的 `-Djava.library.path=${natives_directory}/java` 是否在所有 26.x 一致；若不一致，按 manifest 动态推导。
3. `default-user-jvm` 的 `-XX:+UseCompactObjectHeaders` 在 Java 25 早期版本是否可用；维护兼容表。
4. Forge 1.20.1 的 `-DignoreList` 是否需要按文件名重写（HMCL 有 path-sensitive 修复）；M0 先原样传递，若启动失败再加。
5. 旧版 LWJGL 中文路径：M0 先在 Windows 记录现象，不实现 JLW。
