# 12 · 兼容性矩阵与测试基线

> 目的：把"支持哪些版本/加载器/平台"变成可执行的测试矩阵，避免"声称支持但没人测"。
> 版本数据采样于 2026-10-20；加载器版本会更新，测试时以"当时最新 stable/recommended"为准，并记录到测试报告。

## 1. 支持等级

| 等级 | 含义 | 要求 |
|---|---|---|
| **S0** | 每次发布必测 | 真实安装 + 启动 + 基本交互 |
| **S1** | 发布前冒烟 | 真实安装或解析 + 启动到主菜单 |
| **S2** | 尽力支持 | 解析/单元测试通过；不保证实机 |
| **Out** | 不支持 | 明确报错，不静默失败 |

## 2. 平台

| 平台 | 架构 | 等级 | 说明 |
|---|---|---|---|
| Windows 10/11 | x86_64 | **S0** | 主力；无控制台窗口；静态 CRT |
| Linux（Ubuntu 22.04/24.04） | x86_64 | **S1** | X11 优先，Wayland 冒烟 |
| macOS 13+ | arm64 | S2 | `.app`；未公证；`-XstartOnFirstThread` |
| macOS 13+ | x86_64 | S2 | 同上 |
| Windows 10/11 | arm64 | S2 | Mojang 有 ARM64 JRE；需验证 natives/OpenGL |
| Linux | arm64 | S2 | Mojang 无 ARM64 JRE；需 Adoptium 兜底 |
| Windows 7/8、32 位 | — | **Out** | 不承诺；报错提示 |

## 3. Minecraft 版本

### 3.1 核心矩阵

| MC | Java（最低/推荐） | assets | 备注 | 等级 |
|---|---|---|---|---|
| **26.3** | 25 / 25 | index `34` | 日期式版本；`default-user-jvm`；natives 子目录 | **S0** |
| **1.21.11** | 21 / 21 | 29 | 1.21 最后一个小版本；Forge/NeoForge 可用 | **S0** |
| **1.20.1** | 17 / 17 | 5 | 模组生态主力；Forge 47.x | **S0** |
| **1.12.2** | 8 / 8 | 1.12 | LaunchWrapper；仅 Forge | **S0** |
| **1.7.10** | 8 / 8 | 1.7.10 | 旧参数；仅 Forge | **S0** |
| 1.21.1 | 21 / 21 | 17 | Forge 52.x / NeoForge 21.1.x | S1 |
| 1.20.6 | 21 / 21 | 19 | 1.20.5+ Java 21 分界 | S1 |
| 1.20.4 | 17 / 17 | 12 | NeoForge 20.4.x | S1 |
| 1.18.2 | 17 / 17 | 8 | | S1 |
| 1.16.5 | 8 / 8 | 5 | Forge 36.x；老模组 | S1 |
| 1.17.1 | 16 / 17 | 7 | 过渡版本 | S2 |
| 1.13–1.15 | 8 / 8 | 对应 | 旧 Forge；modlauncher 早期 | S2 |
| 1.19.x | 17 / 17 | 对应 | | S2 |
| 快照 | 按 manifest | 对应 | 允许启动；不保证模组 | S2 |
| ≤1.6.4 | 8 / 8 | legacy virtual | 设计支持；v1 不测 | Out（v1） |

### 3.2 版本号规则

- **不要把版本号当 semver**：`26.3`、`26.1.2`、`1.21.11`、`23w31a` 都是合法字符串。
- 排序：`releaseTime`（RFC3339）优先；缺失时按清单顺序；不要解析数字。
- Java 需求：优先 `javaVersion.majorVersion`；缺失时按版本段表（1.17→16、1.18–1.20.4→17、1.20.5+→21、26.1+→25）。
- 26.x 的 `default-user-jvm`/natives 子目录按 manifest 动态处理，不硬编码。

## 4. 加载器

### 4.1 版本基线（测试时以最新 stable/recommended 为准）

| MC | Fabric | Quilt | Forge | NeoForge |
|---|---|---|---|---|
| 26.3 | 0.20.1 | 0.20.0-beta.9 | 66.0.13（latest） | 26.3.0.95 |
| 1.21.11 | 0.20.1 | 0.20.0-beta.9 | 61.2.0（recommended）/ 61.2.4 | 21.11.48 |
| 1.21.1 | 0.20.1 | 0.20.0-beta.9 | 52.1.0 / 52.1.20 | 21.1.209 |
| 1.20.1 | 0.20.1 | 0.20.0-beta.9 | 47.4.10（recommended）/ 47.4.28 | 1.20.1-47.1.106（`net.neoforged:forge`） |
| 1.16.5 | 不支持 | 不支持 | 36.2.34 / 36.2.42 | — |
| 1.12.2 | 不支持 | 不支持 | 14.23.5.2859 / 14.23.5.2864 | — |
| 1.7.10 | 不支持 | 不支持 | 10.13.4.1614 | — |

> Fabric meta 对 1.12.2 返回空列表，Quilt 无对应条目；不要生成"Fabric/Quilt 1.12.2"选项。
> Forge 版本来自 promotions（latest/recommended）；NeoForge 来自 maven metadata。
> 这些版本会变；UI 应显示"推荐/最新"，而不是写死单一版本。

### 4.2 加载器能力

| 加载器 | 安装方式 | 是否需要 processors | v1 |
|---|---|---|---|
| Vanilla | 官方 profile | 否 | ✅ S0 |
| Fabric | meta profile JSON | 否 | ✅ S0 |
| Quilt | meta profile JSON | 否 | ✅ S1 |
| Forge 1.13+ | installer `install_profile` | ✅ 6 个客户端 processor（1.20.1） | ✅ S0 |
| Forge ≤1.12.2 | installer `versionInfo` | 否 | ✅ S0 |
| NeoForge | 同 Forge | ✅ | ✅ S1 |
| OptiFine | jar patch | — | ❌ v2 |
| LiteLoader | tweaker | — | ❌ v2 |
| Legacy Fabric | 单独 meta | 否 | ❌ v2 |

### 4.3 组合限制

| 组合 | 行为 |
|---|---|
| Fabric + Forge 同实例 | 不支持；UI 不允许 |
| 多个加载器叠加 | 不支持（v1 单加载器） |
| 加载器版本与 MC 不匹配 | 安装前拒绝，显示支持范围 |
| 实例已有模组时切换加载器 | 允许，但警告"模组可能不兼容" |
| Forge 1.17+ + Java 8 | 拒绝（需要 ≥16/17） |
| OptiFine 作为 Forge mod | 允许用户手动放入 mods；启动器不安装 |

## 5. Java 兼容

| MC / 场景 | 最低 | 推荐 | 硬限制 |
|---|---|---|---|
| 1.7.10–1.12.2 | 8 | 8 | LaunchWrapper 要求 ≤8；Linux x86_64 老版本需 64 位 Java 8 |
| 1.13–1.16.5 | 8 | 8 | ≥8u51 |
| 1.17.x | 16 | 17 | Forge 1.17 建议 16/17 |
| 1.18–1.20.4 | 17 | 17 | Forge 1.18–1.20.4 建议 17 |
| 1.20.5–1.21.11 | 21 | 21 | Forge 1.20.5+ 建议 21 |
| 26.1+ | 25 | 25 | Forge/NeoForge 26.x 建议 25 |
| 32 位系统 | — | — | 仅 Java 8/16/17（按平台）；不承诺大内存 |

- 托管 JRE 优先选用与 `javaVersion.component` 匹配的 Mojang 运行时。
- 用户强制选择不兼容 Java：默认阻止；高级设置可"仍然尝试"（记录警告）。
- Java 25 + 旧 LWJGL 的 `MemoryUtil` 问题：v1 只警告；v2 评估 native patch/agent。

## 6. 资源与目录

| 场景 | 行为 | 等级 |
|---|---|---|
| 1.7+ 对象资源 | `assets/objects/<xx>/<hash>` + index | S0 |
| ≤1.6.4 legacy virtual | `assets/virtual/<id>` 重建 | Out（v1） |
| 共享 `.minecraft` | `shared_dir` 指向已有目录，复用资源 | S1 |
| 实例隔离 | `RunDir = Instance` | S0 |
| 共享游戏目录 | `RunDir = Shared` | S1 |
| 自定义游戏目录 | `RunDir = Custom` | S1 |
| 中文路径 | argv 原样；旧版 natives 用 ASCII 临时目录 | S0（Windows） |
| Windows 长路径 | 检测并提示；不保证 >260 | S2 |

## 7. 账户

| 类型 | v1.0 | 说明 |
|---|---|---|
| 离线账户 | ✅ S0 | 本地/局域网；UUID 按 Java 算法 |
| 微软账户 | ⏳ M3.5 | 待 client id + Minecraft Services allowlist；不阻塞 v1.0 |
| authlib-injector | S2（v1.1） | 外置登录；HTTPS 默认 |
| 多账户 | ✅ S1 | 元数据 + 钥匙串（正版启用后） |

## 8. 整合包

| 格式 | v1 | 说明 |
|---|---|---|
| Modrinth `.mrpack` | ✅ S0 | 无需 API key；文件哈希完整 |
| CurseForge zip | ✅ S1 | 需 API key；无 key 只导入 overrides |
| MultiMC/Prism 实例 | ✅ S1 | `mmc-pack.json` + `instance.cfg` + overrides |
| HMCL 整合包 | S2 | v1.1 |
| 手动整合包 | S1 | 识别 `minecraft/`/`overrides/` |
| Technic/ATLauncher/FTB | Out | 不做 |

## 9. 网络与镜像

| 源 | 用途 | 等级 |
|---|---|---|
| Mojang piston-meta / piston-data | 版本、库、资源、JRE | S0 |
| Fabric / Quilt meta | 加载器 profile | S0 / S1 |
| Forge / NeoForge maven | installer、库 | S0 / S1 |
| BMCLAPI | 镜像 | S1 |
| 自定义镜像 | 用户配置 | S2 |
| Modrinth API | 整合包/内容 | S1 |
| CurseForge API | 整合包 | S1（需 key） |
| authlib-injector | 外置登录 | S2 |

## 10. 测试矩阵

### 10.1 单元/快照（每次 CI）

| 版本 | 测试 |
|---|---|
| 1.7.10 | 旧参数、assets、libraries |
| 1.12.2 | LaunchWrapper、Java 8、Forge profile |
| 1.20.1 | 现代参数、logging、natives、Forge 47 |
| 1.21.11 | Java 21、Fabric |
| 26.3 | default-user-jvm、natives 子目录、Java 25 |
| 加载器 profile | Fabric/Quilt/Forge/NeoForge 合并 |
| 规则 | os/arch/features/versionRange |
| 错误 | 缺 mainClass/client jar/library、环、超深 |

### 10.2 真实安装/启动（发布前）

| # | 场景 | 期望 |
|---|---|---|
| 1 | 空目录安装 1.20.1 Vanilla，启动到主菜单 | S0 |
| 2 | 空目录安装 26.3 Vanilla，启动到主菜单 | S0 |
| 3 | 1.20.1 + Fabric 0.20.1 | S0 |
| 4 | 1.21.11 + Fabric 0.20.1 | S0 |
| 5 | 1.20.1 + Forge 47.4.10 | S0 |
| 6 | 1.12.2 + Forge 14.23.5.2859 | S0 |
| 7 | 1.7.10 + Forge 10.13.4.1614 | S0 |
| 8 | 1.21.1 + NeoForge 21.1.209 | S1 |
| 9 | 26.3 + NeoForge 26.3.0.95 | S1 |
| 10 | 26.3 + Forge 66.0.13 | S1 |
| 11 | 导入 mrpack（含 Fabric） | S0 |
| 12 | 导入 CurseForge 包（有/无 key） | S1 |
| 13 | 导入 MultiMC 实例 | S1 |
| 14 | 取消安装/启动；断电恢复 | S0 |
| 15 | 中文用户名/路径 | S0（Windows） |

### 10.3 夜间（允许网络）

- 解析当天最新 release / snapshot，检查端点与字段是否变化。
- 下载 1.20.1 的 manifest + 版本 JSON（不下载大文件），断言解析。
- Forge/NeoForge installer 的 `install_profile` 结构检查（不跑 processors）。
- 失败只报警，不阻塞合并；连续 3 天失败升级为 issue。

## 11. 已知不支持与错误提示

| 场景 | 提示 | 错误码 |
|---|---|---|
| Fabric/Quilt + 1.12.2 | 该加载器不支持此版本，请使用 Forge | `LOADER_UNSUPPORTED` |
| OptiFine 安装 | v1 不支持自动安装；可手动放入 mods | `LOADER_UNSUPPORTED` |
| Forge + Java 8（1.17+） | 需要 Java 16/17/21 | `JAVA_INCOMPATIBLE` |
| 32 位 Java + 大内存 | 32 位 Java 无法使用该内存设置 | `JAVA_INCOMPATIBLE` |
| 缺 client jar | 请先安装原版 | `CLIENT_JAR_MISSING` |
| 加载器 profile 缺库 | 请修复/重装加载器 | `LIBRARY_MISSING` |
| 快照不保证模组 | 仅保证启动 | 警告 |
| 旧版中文路径崩溃 | 已知 LWJGL 限制；尝试 ASCII 路径 | `NATIVES_EXTRACT` / 警告 |

## 12. 维护规则

1. 每次发布前更新本表：记录实际测试的 MC/loader/Java 版本。
2. 新 MC 版本发布后 7 天内：加入 S2 解析测试；确认 Java 需求与参数变化；再决定是否升 S1/S0。
3. 加载器版本更新：fabric/forge/neoforge 的 latest/recommended 变化不改变代码，只更新测试基线。
4. 端点变化：先修镜像/解析，再更新文档。
5. 不把"未测试"写成"支持"；S0/S1 必须有测试记录链接。
