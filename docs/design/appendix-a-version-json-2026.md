# 附录 A · 2026 年 version JSON 实测记录

> 采样时间：2026-10-20（Asia/Shanghai）。Minecraft 已改用**日期式版本号**（`26.3`），并引入若干新字段/新路径。
> 本附录记录实测数据，作为 [04](04-version-model.md) / [05](05-launch-pipeline.md) 的证据；端点会变化，代码必须数据驱动。

## 1. 版本清单

请求：`GET https://piston-meta.mojang.com/mc/game/version_manifest_v2.json`

```json
{
  "latest": { "release": "26.3", "snapshot": "26.4-snapshot-4" },
  "versions": [
    { "id": "26.4-snapshot-4", "type": "snapshot", "url": "https://piston-meta.mojang.com/v1/packages/650b.../26.4-snapshot-4.json", "releaseTime": "2026-10-13T12:05:46+00:00", "sha1": "650b...", "complianceLevel": 1 }
  ]
}
```

- 共 **919** 个版本条目（含远古 alpha/beta）。
- 近期正式版顺序：`26.3, 26.2, 26.1.2, 26.1.1, 26.1, 1.21.11, 1.21.10, 1.21.9, 1.21.8, ...`。
- 含义：**不能**把版本号解析为 `1.x`；排序必须用 `releaseTime`，展示用原字符串。

## 2. 26.3 的 version JSON

请求：`GET https://piston-meta.mojang.com/v1/packages/11f1.../26.3.json`

| 字段 | 值 |
|---|---|
| `id` | `26.3` |
| `type` | `release` |
| `mainClass` | `net.minecraft.client.main.Main` |
| `javaVersion` | `{ "component": "java-runtime-epsilon", "majorVersion": 25 }` |
| `assets` | `34` |
| `assetIndex` | `{ "id": "34", "sha1": "e787e8...", "size": 597035, "totalSize": 484829952, "url": "..." }` |
| `downloads.client` | `{ "sha1": "e877b6...", "size": 41483720, "url": "https://piston-data.mojang.com/v1/objects/e877b6.../client.jar" }` |
| `libraries` | 114 个 |
| `minimumLauncherVersion` | 21 |
| `arguments` | `default-user-jvm`, `game`, `jvm` |

### 2.1 `arguments.default-user-jvm`（新字段）

```json
[
  { "value": ["-Xms2G", "-Xmx4G", "-XX:+UseCompactObjectHeaders", "-XX:+AlwaysPreTouch", "-XX:+UseStringDeduplication"] },
  {
    "rules": [
      { "action": "allow", "os": { "name": "osx" } },
      { "action": "allow", "os": { "name": "linux" } },
      { "action": "allow", "os": { "name": "windows", "versionRange": { "min": "10.0.17134" } } }
    ],
    "value": ["-XX:+UseZGC"]
  },
  {
    "rules": [
      { "action": "allow", "os": { "name": "windows", "versionRange": { "max": "10.0.17134" } } }
    ],
    "value": ["-XX:+UnlockExperimentalVMOptions", "-XX:+UseG1GC", "-XX:G1NewSizePercent=20", "-XX:G1ReservePercent=20", "-XX:MaxGCPauseMillis=50", "-XX:G1HeapRegionSize=32M"]
  }
]
```

要点：

- 新增 `os.versionRange`（min/max），规则引擎必须支持；Windows 版本比较用 build 号。
- `-XX:+UseCompactObjectHeaders` 仅 Java 25+ 支持；若用 Java 21 启动必须移除。
- 2G/4G 默认内存对 8 GB 以下机器偏大，启动器需钳制（见 05 §8.4）。
- `default-user-jvm` 不参与父/子拼接：子有则用子。

### 2.2 JVM 参数与 natives 路径

```text
-Djava.library.path=${natives_directory}/java
-Djna.tmpdir=${natives_directory}/jna
-Dorg.lwjgl.system.SharedLibraryExtractPath=${natives_directory}/lwjgl
-Dio.netty.native.workdir=${natives_directory}/netty
--enable-native-access=ALL-UNNAMED
--add-exports java.base/jdk.internal.misc=ALL-UNNAMED
-XX:StackShadowPages=32
...
-cp
${classpath}
```

- **`${natives_directory}/java`**：26.x 要求把 native 库解压到 `<natives>/java`，否则 `java.library.path` 找不到。
- 对比 1.21.11 / 1.20.1：`-Djava.library.path=${natives_directory}`（无子目录）。因此必须**动态解析**，不能硬编码。
- `--enable-native-access` / `--add-exports` 是 Java 25 所需的模块参数，按 manifest 原样传递。
- `default-user-jvm` 出现在 `arguments` 顶层，与 `game`/`jvm` 并列。

### 2.3 库与 natives

- 26.3 的 114 个库中，native 以**带 classifier 的独立库**出现，例如：
  ```json
  { "name": "org.lwjgl:lwjgl:3.4.3:natives-windows", "downloads": { "artifact": { "path": "org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3-natives-windows.jar", ... } } }
  ```
- **没有**旧式 `natives` 映射；`natives` 字段只在旧版本（如 1.12.2）出现。
- 现代 native 库也不能进 classpath（它们是纯 native jar），需按 classifier 识别并解压。
- 存在 `natives-windows-arm64`、`natives-macos-arm64`，按架构选择。

### 2.4 资源

- 索引 `34`：597 KB，对象总量约 **484.8 MB**（`totalSize`）。
- 对象路径 `assets/objects/<xx>/<sha1>`；索引 URL 自带 SHA1。
- 安装时需要并发下载数千个小文件；单主机限流很重要。

## 3. Java 运行时清单

请求：`GET https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json`

顶层平台 key：

```text
gamecore, linux, linux-i386, mac-os, mac-os-arm64, windows-arm64, windows-x64, windows-x86
```

组件（以 `windows-x64` 为例）：

| 组件 | 版本 | 用途 |
|---|---|---|
| `java-runtime-epsilon` | 25.0.1 | MC 26.1+ |
| `java-runtime-delta` | 21.0.7 | MC 1.20.5–1.21.x |
| `java-runtime-beta` | 17.0.x | 1.18–1.20.4 |
| `java-runtime-alpha` | 16.0.x | 1.17 |
| `java-runtime-gamma` | 17.0.x | 1.20.1 等 |
| `jre-legacy` | 8u51 / 8u202 | ≤1.16.5 |

- 每个组件有 `manifest`（URL + sha1 + size）与 `version`。
- `linux-arm64` **不存在**；Linux ARM64 需 Adoptium 等兜底。
- `gamecore` 是主机平台，忽略。
- 运行时 manifest 的 `files` 含 `directory`/`file`/`link`，`downloads.raw` 与 `downloads.lzma`，`executable` 标记。
- 实测 Windows x64 的 `java-runtime-epsilon`：493 个文件，raw 总量约 105 MB；`bin/java.exe` 可执行。

HMCL 把此 URL 作为常量（`MojangJavaDownloadTask.JAVA_LIST_URL`），说明其稳定性可接受；设计中仍设为可配置。

## 4. 加载器端点实测

| 端点 | 结果 |
|---|---|
| `GET https://meta.fabricmc.net/v2/versions/loader/1.21.11` | 200，255 个 loader；最新 `0.20.1`（stable=false），常用 stable 仍可选 |
| `GET https://meta.fabricmc.net/v2/versions/loader/1.21.11/0.18.1/profile/json` | 200；`mainClass=KnotClient`，8 个库，`inheritsFrom=1.21.11`，jvm 参数含 `-DFabricMcEmu= net.minecraft.client.main.Main `（带空格） |
| `GET https://meta.quiltmc.org/v3/versions/loader/1.21.11` | 200，返回 loader 列表 |
| `GET https://files.minecraftforge.net/.../promotions_slim.json` | 200；1.20.1 recommended `47.4.10`，latest `47.4.28`；1.21.1 latest `52.1.20` |
| `GET https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml` | 200；latest `26.3.0.95` |
| Fabric profile 库 | 无 `downloads`，有 `url` + `sha1/sha256/sha512` → 下载器必须支持两种形态 |

## 5. Forge installer 实测（1.20.1-47.4.10）

- `forge-1.20.1-47.4.10-installer.jar`：约 **6.08 MB**。
- `install_profile.json`：
  - `spec: 1`，`version: 1.20.1-forge-47.4.10`，`minecraft: 1.20.1`。
  - `data` 键：`MAPPINGS, MOJMAPS, MERGED_MAPPINGS, BINPATCH, MC_UNPACKED, MC_SLIM, MC_SLIM_SHA, MC_EXTRA, MC_EXTRA_SHA, MC_SRG, PATCHED, PATCHED_SHA, MCP_VERSION`。
  - `BINPATCH = { client: "/data/client.lzma", server: "/data/server.lzma" }`。
  - `MC_UNPACKED = "[net.minecraft:client:1.20.1-20230612.114412:unpacked]"`（伪 maven 坐标）。
  - `processors`：10 个；`sides: ["server"]` 的跳过；客户端实际 6 个：
    1. `MCP_DATA`（installertools）
    2. `DOWNLOAD_MOJMAPS`
    3. `MERGE_MAPPING`
    4. `jarsplitter`（产出 slim/extra）
    5. `ForgeAutoRenamingTool`
    6. `binarypatcher`（应用 `client.lzma`）
  - `libraries`：36 个，多数带 `downloads.artifact`。
  - installer 内还含 `maven/` 内嵌库、`version.json`、`data/win_args.txt`、`data/unix_args.txt`。
- `version.json`：
  - `id: 1.20.1-forge-47.4.10`，`inheritsFrom: 1.20.1`，`mainClass: cpw.mods.bootstraplauncher.BootstrapLauncher`。
  - jvm 参数含 `-DignoreList=...`、`-DlibraryDirectory=${library_directory}`、`-p ...${classpath_separator}...`、`--add-modules ALL-MODULE-PATH`、`--add-opens/--add-exports`。
  - **没有 `-cp`**：依赖继承自原版 1.20.1 的 `-cp ${classpath}`。合并顺序（父先）保证它存在；若 profile 无父，启动器需兜底追加 `-cp`。
  - `data/win_args.txt` 额外包含 `-DlegacyClassPath=...`；启动器若直接消费 `version.json`，BootstrapLauncher 会回退到 `java.class.path`（即 `-cp`），因此无需该属性。

### 5.1 NeoForge（21.1.209）

- installer 约 6.95 MB；`install_profile.json` 结构同上，processors 10 个，data 键相同。
- processor jar：`net.neoforged.installertools:installertools:2.1.2`。
- 说明：Forge 与 NeoForge 可共用同一套 processor 执行器，只更换 maven/版本解析。

## 6. 镜像实测

| 请求 | 结果 |
|---|---|
| `https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json` | 302 → 对象存储，200；内容与官方一致（latest 26.3） |
| `https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader/1.20.1` | 200，约 400 KB |
| `https://bmclapi2.bangbang93.com/maven/net/minecraftforge/forge/1.20.1-47.4.10/forge-...-installer.jar` | 302 → USTC 镜像，200，6,078,070 B |

含义：

- 必须允许跨域重定向（官方 → 对象存储/镜像站）。
- 不能假设镜像域名是最终来源；校验仍以官方哈希为准。
- 302 后的 URL 可能带签名参数，日志里应去掉 query。

## 7. HMCL 的兼容处理（可验证的参考）

- `GameJavaVersion` 映射：`>= 26.1 → Java 25`、`>= 1.20.5 → 21`、`>= 1.18 → 17`、`>= 1.17 → 16`、`>= 1.13 → 8`。
- `DefaultLauncher` 显式处理 `-Djava.library.path=${natives_directory}/<sub>`：解析子目录并只把 native 解压到那里（并防目录穿越）。
- `GameJavaVersion.JAVA_25` 使用组件 `java-runtime-epsilon`。
- Fabric 的 `-DFabricMcEmu= net.minecraft.client.main.Main ` 作为单个 JVM 参数解析，不能按空格切分。

## 8. 对 nailong 的硬性要求

1. **版本号是字符串**，排序用 `releaseTime`；UI 不假设 `1.x`。
2. **解析 `default-user-jvm`**，并按 Java major 过滤不兼容标志。
3. **natives 目标目录动态推导**（`${natives_directory}` 可能带子目录）。
4. **规则支持 `os.versionRange`、`os.arch`、`features`**。
5. **native 库识别**：旧式 `natives` 映射 + 新式 `natives-*` classifier。
6. **Forge profile 无 `-cp`**：依赖继承；自定义 profile 需兜底。
7. **Java 组件名**来自 manifest，不要硬编码 `java-runtime-gamma`。
8. **Mojang JRE 清单 URL** 作为可配置常量，并准备 Adoptium 兜底（Linux ARM64）。
9. **镜像允许重定向**，但每跳校验协议；最终内容用官方哈希校验。
10. **夜间端点检查**：26.x 格式仍可能变化，回归测试要覆盖最新正式版。

## 9. 复现命令

```bash
curl -s https://piston-meta.mojang.com/mc/game/version_manifest_v2.json | jq '.latest, .versions[0]'
curl -s https://piston-meta.mojang.com/v1/packages/11f1aefe473eefba0af62e3286c33b692807b2eb/26.3.json | jq '{id, javaVersion, assets, libraries: (.libraries|length), args: (.arguments|keys)}'
curl -s https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json | jq 'keys'
```

> 上述 JSON 的哈希前缀会随版本更新变化；`26.3.json` 的 URL 来自当天的版本清单，不应写死。
