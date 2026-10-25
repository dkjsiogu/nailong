# 14 · 原型实现状态（0.1 → 0.23.0）

> 日期：2026-10-25
> 对应发布：GitHub Release `v0.23.0` 的 `nailong.exe`、`nailong_0.23.0_amd64.deb`
> 目的：把「设计稿」和「已经跑起来的代码」分开记账，避免把计划当成已完成。

## 1. 一句话结论

**垂直切片已经打通：Windows/Linux 上可以安装 Minecraft 1.21.1，并选择 Fabric / NeoForge / Forge / 原版，用离线账户启动到主菜单。**

0.4.2 把 Forge/NeoForge 的安装数据从“整个安装器”改成“按需读取核心条目”：首次安装只取约 3 MB，而不是 6–9 MB；不支持 Range 的镜像自动改用官方源或完整包兜底。

0.4.3 补上两项基础体验：加载器依赖的原版不再出现在主版本列表里（管理页仍可见、可删）；已完整安装的加载器再次点安装会直接复用，不再下载补丁或重跑 processor。3 MB 客户端补丁也改为 4 段并行读取。

0.4.4 把发布体积从约 12 MB 降到约 9 MB（`opt-level=z` + fat LTO + `panic=abort`，保留静态 CRT 和系统字体），并把精简安装包缓存到机器级目录；同一补丁在第二个数据目录中直接复用。

0.4.5 把安装从串行改成依赖感知的流水线：版本 JSON 写好后，游戏资源、client/libraries、加载器补丁、processor 并行推进；processor 不再等资源下完才开始，而是只等自己依赖的 client/libraries。

0.5.0 加入 CAS 与本机 patched client 复用：先只读 loader 元数据（约 130 KB），找到官方 / Prism / MultiMC / Modrinth / 相邻 `.minecraft` 中同版本的 patched client，校验后硬链接到当前实例，跳过 3 MB 补丁和全部 processor。Forge 用 `PATCHED_SHA` 严格校验；NeoForge 上游没有输出哈希，按 JAR 结构校验。

0.5.1 把下载器继续往下拆：单个大文件（如 26 MB client jar）自动 4 段 Range 并行，段文件稳定命名、可续传；主源失败自动切备用源；单次传输超时从 60 秒放宽到 180 秒。

0.6.0 统一整合包导入：按 ZIP 签名识别 Modrinth `.mrpack`、CurseForge `manifest.json`、Prism/MultiMC `mmc-pack.json`；分别走直链/Range、CurseForge 文件解析、组件加载器安装，最后解压 overrides/.minecraft。无法识别的格式明确报错。

0.7.0 补齐 MCBBS、HMCL、ATLauncher：MCBBS 用 addons + files + overrides；HMCL 读 `modpack.json` + `minecraft/pack.json`，ATLauncher 读 `instance.json`，把内嵌 manifest 转成实例 profile 并安装基础版本。CurseForge 支持 `CURSEFORGE_API_KEY` 官方 API 校验 SHA1/大小；所有下载增加 Content-Length 大小检查。

0.7.1 增加设置页：CurseForge API Key 可掩码查看、保存、清除，写入 `<数据目录>/nailong.json`（Unix 0600），环境变量优先；无 key 的 cfwidget 回退不变。设置页在版本列表为空时也可进入。

0.7.2 修正 CurseForge 合规路径：官方 API 的 `downloadUrl` 为 null（作者关闭第三方分发）时停止导入并列出文件，不再手拼 CDN 链接；无 key 回退改用官方下载页。元数据查询加入 120 ms 间隔，并支持用 `NAILONG_CURSEFORGE_API_KEY` 在发布构建中内置默认 key（运行时环境变量和设置文件仍优先）。

0.7.3 增加安装取消：进程级取消标志贯穿 HTTP/HTTP Range、分段下载、blob 落盘、整合包解压和 Forge/NeoForge processor；GUI 进度页有取消按钮，取消后保留已下载部分并回到“继续安装”。Forge/NeoForge 开始 processor 前会写 `.nailong-incomplete` 标记，成功后删除，health 不会把半成品当完整安装。当前是单任务全局标志，不是 async `CancellationToken`；正在进行的单个 HTTP 请求要等它返回或超时。

0.7.4 的官方构建内置 CurseForge 默认 key：CI 从 `NAILONG_CURSEFORGE_API_KEY` 注入，普通用户无需填写。运行时环境变量和设置文件仍优先；内置 key 在公开二进制中可被提取，只能当共享默认配额。无 key 构建继续使用 cfwidget + 官方下载页回退。

0.7.5 简化凭据路径：移除设置页里的 API key 输入和 `nailong.json` 存储，官方构建默认 key 直接可用；维护者仍可用环境变量 `CURSEFORGE_API_KEY` 覆盖。key 由 `build.rs` 在编译期 XOR 混淆后写入二进制，避免 `strings` 直接看到；这不是加密，仍可被还原。

0.8.0 增加 Mod 检索与下载：选中 Fabric/NeoForge/Forge 版本后锁定 MC/加载器，搜索 Modrinth + CurseForge，查看兼容文件，解析 required/optional/incompatible 前置，下载到该版本的独立 `mods/`。普通版本首次下载会写 `.nailong-isolated`，之后 `gameDir` 指向 `versions/<id>/`；根目录旧存档不迁移。

0.9.0 把下载中心补齐：资源包/光影搜索与下载、结果分页、已安装内容索引（扫描、启用/禁用、删除、重复检测、更新检查/一键更新）、本地 `.jar`/`.zip` 导入。Mod 更新会按最新兼容版本重解依赖并把旧文件从索引/目录移除；CurseForge 依赖仍只有 `modId`，因此只能选目标版本下最新的兼容文件。

0.10.0 把下载中心从“先搜索”改成“先浏览”：打开就是品类胶囊（每类内容 8–10 个品类）+「热门」「最新」两排卡片；卡片带项目图标、下载量和一个 `+`，点一下即装最新兼容版本和必需前置（`mods::install_project`），装完卡片变「已装」。搜索保留在右上角，结果复用同一套卡片。图标走后台线程拉取、`image` 解码后在 UI 线程建纹理，并做了占位色块兜底。

0.11.0 把品类换成 CurseForge 的真实目录：运行时拉 `/v1/categories`（每种内容一次），用归一化匹配翻成中文标签并映射到 Modrinth 分类 slug，未收录的保留 CurseForge 原名；新增来源切换（全部 / Modrinth / CurseForge），浏览时把 `categoryId` 直接传给 CurseForge 搜索接口。下载路径不变，仍是启动器解析文件地址、校验后落盘。0.11.2 起品类默认只铺 12 个，其余收进「更多品类 +N」，避免 CurseForge 三四十个分类把浏览页顶满。

0.23.0 修玩家第一眼就会碰到的三类毛刺：① Windows 打开登录链接不再走 `cmd /C start`，因为 cmd 会把 URL 里的 `&` 当命令分隔符，登录链接从第一个 `&` 截断，微软报 AADSTS900144（scope 丢失）；改用 `rundll32 url.dll,FileProtocolHandler` 直接传完整 URL。② 登录窗口的“取消”不再调用全局安装取消标志；安装/内容任务结束和进入发现/搜索前会重置，修掉“发现 → 操作已取消”的误报。③ 启动台不再在每次点击时同步扫描全部版本和 world，首页统计与最近内容按页缓存，版本页统计也缓存；页面切换从 260 ms 收到 180 ms。发现/搜索结果在 1120 宽从 3 列变 4 列（卡片最小宽 200），同屏多一个项目。登录失败只显示一句可行动的话，原始 AADSTS 进日志。

0.22.0 把加载器状态纳入恢复点：内容目录就是版本目录时，`<profile>.json` 与 `<profile>.jar` 会和 Mod/配置一起进对象库；恢复时先按清单删掉这两个文件再还原，避免从加载器版本退回原版后留下旧的补丁 jar。重装/修复已有加载器前也会自动留一份。共享 `libraries/` 不复制——nailong 不会删除旧库，旧加载器仍能解析；世界存档继续走独立快照。默认实例（内容写进版本目录）覆盖，自定义外部 `gameDir` 的实例只恢复内容。

0.21.0 把“装之前先看一眼”和“第一次启动”补上：内容卡片的一键安装、详情页安装按钮都先弹确认清单——项目、类型、安装到哪个版本、实际写入目录、版本/文件、预计大小、会一起装的前置，以及“安装前自动创建恢复点；装坏了在「内容 › 恢复点」退回来”。确认后才走原来的解析/下载路径，取消不写盘。首次启动不再直接甩安装表单，而是三步引导：Java 21+ 检测与下载页/重新检测、游戏目录透明展示、安装第一个版本或导入整合包；没有版本时启动台就是这张引导页。设置仍收在侧边栏。0.21.0 的恢复点范围仍是内容/配置/索引，加载器二进制和版本 JSON 留到后续。

0.20.0 实现实例级内容恢复点（`core::rollback`）：把 `mods`、`config`、`resourcepacks`、`shaderpacks` 和 `options.txt` / `.nailong-content.json` 按整文件 SHA-1 存进 `backups/instance/objects`，未改动的 jar 跨恢复点复用。安装、更新、导入本地内容和删除内容前自动创建恢复点；创建失败会中止操作，不静默继续。内容页新增「恢复点」弹窗：手动创建、恢复、删除，恢复前先为当前状态留一份，恢复时逐个校验对象哈希。默认保留最近 10 份；世界存档仍走独立的块级快照。恢复点不包含加载器二进制和版本 JSON，这部分留到后续迭代。

0.19.0 按产品 brief 重构启动台：首页从“仪表盘”收成单一主行动——当前版本、开始游戏、版本详情、最近世界和最近内容；账户、玩家名、内存、Java、游戏目录、数据目录和日志全部移进侧边栏底部「设置」，玩家名与内存写入数据目录 `ui.json`，重启后保留。删掉首页的账户卡、四项统计、状态环、快捷操作和日志折叠，把“管理”从每日路径里拿掉；空存档/空内容仍就地给下一步。新增设置弹窗截图，产品 brief 落在 `docs/product/00-brief.md`。

0.18.1 做字体与控制细节：单独加载 Noto Sans CJK Bold / 微软雅黑 Bold 作为 `bold` 字体族，标题、列表主行、侧边栏当前项和主按钮不再用常规字重冒充粗体；主按钮从“黄橙渐变胶囊”改成哑光琥珀，窄渐变、1 px 深琥珀描边、内侧顶部 1 px 高光、底部 2 px 内阴影、两层暖棕投影，并支持播放/下载字形；次按钮统一为白底、暖色描边和极浅接触阴影，悬停/按下都有独立状态。项目卡片、详情安装按钮、欢迎页和版本页动作行同步更新；18 张截图与 GIF 重拍。实测 GUI 空闲 RSS 约 157 MB（字体增加约 22 MB），这是当前为字重付出的成本。

0.18.0 按玩家视角重排 GUI 的信息架构：用常驻侧边栏收拢四个一级入口——启动台、版本、内容、存档。内容库把「发现」和「已安装」合并成同一页的两种模式，安装目标、类型和搜索都留在页头；版本页删掉「内容 / 存档」按钮，只保留启动、补全、打开目录和删除；存档升为一级页面，自带版本选择器，不再从版本页钻进去，也就没有“回退到版本管理”的路径。安装页补上「‹ 版本」局部返回；没有版本时启动台有欢迎空状态。视觉从深色蓝墨换成奶油白 + 白卡片 + 橙黄强调，状态环、卡片悬停、模态遮罩和危险确认全部按浅色重新调对比度。新增 900×620 侧边栏 rail 布局、版本页窄窗换行、存档工具条换行；18 张截图和动效 GIF 全部重拍。

0.17.0 在 0.16 的设计系统上补光感与动效：`motion` 模块提供渐变圆角、径向光晕、卡片阴影、呼吸主按钮、状态环和带扫光的进度条；页面切换用 260 ms 的暗场遮罩淡入，卡片 hover 抬升 3 px 并亮起描边，Tab/主按钮带缓慢呼吸光，弹窗 200 ms 淡入，开关保持 120 ms 滑块动画；深色底改为偏蓝墨色，新增顶部强调线、页面标题金色竖条、图标渐变和头像金环。动效演示 GIF 与 18 张截图在 `docs/screenshots/`。

0.16.0 重做整个 GUI 的视觉与信息结构：统一色板、字号、间距和组件（`docs/design/15-ui-design-system.md`），主界面、安装、版本管理、下载中心（固定头部 + 浏览/搜索/详情/内容管理）、存档、进度页和账户/登录弹窗全部换到同一套卡片、列表、开关与按钮；内容管理新增筛选、启用开关、更新横幅、逐项更新、行内删除确认；项目详情改为 Hero + 安装面板 + 版本信息/前置 + 版本记录的双栏布局；补齐空状态、失败提示和 900×620 窄窗口布局。截图见 `docs/screenshots/`。

0.15.0 把存档备份换成 git 式快照：`core::snapshots` 用内容定义分块（32 字节滚动哈希、64 KB–4 MB 块）把每个文件切成块，按 SHA-1 存进 `backups/repo/objects/`，快照只记路径和块哈希；第二次备份只写改动过的块。支持恢复（目标已存在时改名，不覆盖）、删除、按份数 prune（顺带 GC 无引用对象）。自动备份按 `backups.json` 的间隔（15/30/60/120 分钟）和保留份数在后台线程跑，实测启动后自动生成第 3 份快照并写回 `last_run`。

0.14.0 把下载中心和版本管理按“先选目标版本”重做：右上角「安装到：<已装版本>」决定浏览/搜索的 MC 版本与加载器，`+` 直接写进该版本的 `mods/`；另有「只浏览」模式不要求安装目标。版本管理里每个版本新增「内容」和「存档」：存档页列出世界（大小/最近修改）、单个或全部备份成 zip、恢复、删除，备份落在 `<gameDir>/backups/`；资源包/光影与 Mod 一样可启停、删除、查看大小、打开目录。库层新增 `core::saves`（列表、备份、恢复、删除，带路径穿越防护与 zip 根目录约定）。

0.13.0 接上微软正版登录：Mojang 的 AppID allowlist 已批，采用浏览器授权码 + PKCE（回环 `http://localhost:39123/callback`，端口占用时退动态端口），走 MSA → Xbox Live → XSTS → `login_with_xbox` → 所有权校验 → profile，令牌写 `account.json`（0600）并在启动前自动刷新；启动参数注入 `--accessToken` / `--uuid` / `--xuid`。设备码被该 Entra 应用配置拒绝（AADSTS70002 要求 public client），所以主路径用 PKCE；设备码代码保留但未启用。0.13.1 修正回环地址：Azure 里注册的是 `http://localhost`（根路径），登录前会先用 `prompt=none` 探测 `http://localhost:<port>/`、`/callback`、`127.0.0.1` 等候选，选微软接受的那个，回调解析也接受根路径 `/?code=...`。0.13.2 把探测改成跟完整跳转链（login.live.com 这一跳才会真正校验 redirect_uri），并在 Azure 未注册 `http://localhost` 时直接给出可操作提示。

0.12.0 让浏览不再绑定已装版本：版本和加载器都可选「全部」，核心把 `mc`/`loader` 改成可选，Modrinth 的 `versions`/加载器 facet 与 CurseForge 的 `gameVersion`/`modLoaderType` 都按需追加；没有已装版本时也能从安装页进下载中心纯逛（此时 `+` 会提示先装版本）。卡片加了简介（名称 + 两行摘要 + 下载量），详情页补作者和完整摘要。

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
| 整合包导入 | `core::install::modpack` | 完成（6 种格式） | Modrinth、CurseForge、Prism/MultiMC、MCBBS、HMCL、ATLauncher；真实 CurseForge 文件 + 各格式合成包验证 |
| 下载中心 | `core::mods` / `core::installed` + `nailong-ui` | 完成 | Mod/资源包/光影检索；分页；真实 API；依赖解析；已安装扫描/启停/删除/重复检测/更新；本地导入 |
| 浏览式发现 + 一键安装 | `core::mods::browse` / `install_project` + `nailong-ui` | 完成 | 品类胶囊 + 热门/最新两排卡片；图标异步加载；卡片 `+` 一键装最新兼容版 + 必需前置；Xvfb 下点击真实安装 fabric-api 并写索引 |
| CurseForge 真实品类 + 来源切换 | `core::mods::curseforge_categories` / `CategoryFilter` + `nailong-ui` | 完成 | 运行时拉 `/v1/categories`；品类翻中文并映射 Modrinth slug；`categoryId` 传给 CF 搜索；全部/Modrinth/CurseForge 切换 |
| 无版本/无加载器浏览 | `core::mods::search` / `browse` + `nailong-ui` | 完成 | 版本、加载器都可为空；纯浏览不要求已安装版本；卡片带简介；联网测试覆盖“无过滤浏览” |
| 版本目标 + 存档管理 | `core::saves` / `core::snapshots` + `nailong-ui` | 完成 | 「安装到」锁定目标版本并直写其目录；存档列表；git 式快照（CDC 分块去重）、恢复、删除、prune；自动备份可配间隔/保留份数 |
| Windows 便携目录 | `core::paths` | 完成 | 默认使用 exe 同目录 `.minecraft` |
| CLI（list/java/info/install/launch） | `nailong-cli` | 完成 | 打包产物冒烟 |
| 实例内容恢复点 | `core::rollback` + `nailong-ui` | 完成（内容/配置/加载器版本文件） | 整文件 SHA-1 对象库；安装/更新/导入/删除/重装加载器前自动留点；恢复前安全留档；6 个单元测试覆盖去重、恢复移除新增 Mod、加载器 JSON/jar 精确还原、旧清单兼容、prune 与路径穿越 |
| 安装确认 | `nailong-ui` | 完成 | 一键安装/详情安装先展示目标、路径、版本、大小、前置与恢复点说明；确认才写盘；Xvfb 实测确认后安装 Sodium 并生成恢复点 |
| 首次启动引导 | `nailong-ui` | 完成 | 无版本时进入三步引导：Java 21+ 检测/下载页/重新检测、目录展示、安装第一个版本或导入整合包；900×620 与 1120×720 复核 |
| GUI（侧边栏、版本、安装/补全、启动、内容库、存档、进度、账户） | `nailong-ui` | 可用 | Xvfb 下走完安装 / 补全 / 启动；安装中取消；四入口点击、内容发现/已安装切换、内容详情/搜索返回、存档版本切换实测；浏览页点卡片 `+` 实际下载并落索引；实际 NeoForge 到主菜单；0.23 复测登录取消后进发现不再误报取消，版本→启动台切换正常 |
| 一级信息架构 + 浅色主题 | `nailong-ui` | 完成 | 侧边栏 204/76 两档；内容合并发现/已安装；存档一级页 + 版本选择；版本页移除内容/存档入口；900×620 与 1120×720 截图复核 |
| 版本管理 | `nailong-ui` | 完成 | 基础版本隐藏、版本/整合包删除、被依赖保护 |
| CAS / patched client 复用 | `core::blob` / `core::install::reuse` | 完成（第一阶段） | client/libraries 新文件走 blob+硬链接；复用后跳过补丁与 processor；Forge 实测 5.3s、NeoForge 3.7s |
| 大文件分段下载 | `core::http` | 完成 | client jar 4 段并行、段续传、源回退 |
| 重复安装快速路径 | `core::install::forge` / `neoforge` | 完成 | health 通过则 0 下载、0 processor |
| Windows 交叉编译 | `x86_64-pc-windows-gnu` | 通过 | 无 MinGW DLL 依赖，仅系统 DLL |
| 发布包 | GitHub Release `v0.23.0` | 完成（CI 构建） | 单 exe 约 8 MB / deb 约 3.3 MB |

### 2.1 实测记录

- **从零安装**：`nailong-cli --root /tmp/nailong-fresh install neoforge 1.21.1 21.1.209` 退出码 0。
- **启动**：同一目录 `launch neoforge-21.1.209 --username Steve --memory 4096`，日志出现
  `Setting user: Steve`、`Backend library: LWJGL version 3.3.3+5`、`Created: ... texture atlas`。
  无显示器环境用 Xvfb；音频设备缺失只产生 ALSA 警告，不影响进入游戏。
- **GUI**：打包后的 `nailong` 在 Xvfb 中持续运行；通过 xdotool 点击“启动”后游戏进程到达主菜单。
- **体积**：Linux GUI 9.0 MB / CLI 3.3 MB；Windows GUI 7.82 MB（CI 原生 MSVC 构建，无 VCRUNTIME/MSVCP 依赖）。opt-level=z + fat LTO + panic=abort，保留静态 CRT 与系统字体。
- **内存**：Linux + Xvfb + llvmpipe 下 GUI 空闲 RSS 约 **157 MB**（0.18.1 加载真粗体中文字体后增加约 22 MB）。这是软件渲染上界，未达到 120 MB 目标；Windows 真实 GPU 待测。

### 2.2 关键实现细节

- **classpath 特判**：NeoForge 的 `bootstraplauncher` / `modlauncher` 主类下，**不能**把原版 client jar 或 patched client jar 放进 classpath，否则会出现
  `Modules minecraft and _1._21._1 export package ...` 或 `more than one module named neoforge`。
  FML 会自己从 `libraries` 找到 patched client 与 universal jar。代码里对应
  `remove_classpath_args` 之后对 `resolved.client_jar` 的剔除。
- **离线 UUID**：`MD5("OfflinePlayer:" + name)`，再置 version=3、variant=2；与 Java 的
  `UUID.nameUUIDFromBytes` 一致。测试向量：`Notch -> b50ad385829d3141a2167e7d7539ba7f`。
- **版本列表**：主界面只展示用户安装的叶子版本；被加载器继承的原版标记为 `hidden`，管理页才列出。删除前检查“被多少版本依赖/被多少整合包使用”，避免把基础版本删掉。
- **安装流水线**：`prepare_version` 先写版本 JSON；随后 `install_assets_with`、`install_client_libs`、`slim::prepare`、`processors::install_client` 按依赖并行，最后统一收束。
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
| 异步 runtime + 事件总线 + 可取消任务 | 阻塞调用 + 线程 + 进程级取消标志（单任务） | 下载、分段、解压和 processor 已可取消；未引入 async runtime |
| 实例（Instance）抽象、配置存储 | 整合包实例已有独立 game dir；普通版本下载 Mod 后启用 `.nailong-isolated`，game dir 指向版本目录 | 版本级隔离已可用；仍无每实例 Java/内存配置 |
| 完整 GUI（路由/向导/下载中心） | 版本页、安装页、版本管理、下载中心（浏览 + 搜索，Mod / 资源包 / 光影，含已安装管理与更新检查） | 无整合包浏览器；更新需手动点，不做后台静默更新 |
| 正版登录 / 自动更新 | 未实现 | 下一步做账户与更新 |
| 微软正版登录 | **已实现（浏览器 PKCE）**，待真实账号走一遍 | 令牌存 0600 文件，未接系统钥匙串 |
| 自动更新 / 签名 / 崩溃报告 | 未实现 | 发布前补齐 |
| 单元/集成测试体系 | **60 个测试**（58 core + 2 UI），另有 7 个真实网络冒烟测试默认忽略（搜索、下载、浏览、一键安装、无过滤浏览、设备码、回环探测） | 缺夹具、mock HTTP、启动 argv 快照、Windows CI |

## 4. 已知问题与风险

1. **Windows 只在 Linux 上交叉编译通过**，还没有 Windows 实机启动记录；OpenGL 驱动、路径、Java 发现都可能出问题。
2. **processor 安装**依赖 `install_profile` 结构和 processor 工具；上游改格式时需要跟进。精简读取还依赖官方 ZIP 结构；不支持 Range 或结构异常时会回退完整包。
3. **离线账户会触发 401**：游戏向 Mojang 请求 user properties 被拒，日志有 `InvalidCredentialsException`，但不阻塞单人游戏；后续可关闭该请求或接正版登录。
4. **取消安装有边界**：下载循环、解压和 processor 会尽快停止；已经在途的单个 HTTP 请求无法立即 abort，最多等到请求超时。processor 会被 kill，已下载的分段/文件保留，下次安装检查补全。
5. **GUI 内存 157 MB**（软件渲染，含约 22 MB 粗体中文字体）高于 120 MB 目标；需要真实 GPU 复测，或改用字体子集/按需加载粗体。
6. **续传范围有限**：普通文件是“已存在且哈希正确则跳过”，不保留半成品；单个大文件有稳定的 `.dlpart-N` 段文件，可续传。安装器核心数据用 HTTP Range 只取所需条目，中断后重跑会重新读取约 3 MB。
7. **无安装锁**：并发启动两个安装可能写同一目录；原型未处理。
8. **资产下载没有虚拟资源映射**（legacy virtual assets）验证；1.21.1 不需要，旧版本可能需要。
9. **下载中心边界**：CurseForge 依赖只有 `modId`，解析时选目标版本下最新的兼容文件，不能保证与作者指定版本完全一致；更新为手动检查/一键更新，不做后台静默更新；版本隔离首次启用不迁移根目录旧存档。

## 5. 下一步（按优先级）

1. **Windows 实机验证**：在一台 Windows 10/11 + Java 21 上跑安装与启动，记录失败点。
2. **processor 校验增强**：outputs SHA1 校验、失败断点续跑。
3. **测试与 CI**：版本 JSON 夹具、规则/合并/argv 快照、mock HTTP（重试/哈希失败）、`fmt/clippy/test`。
4. **托管 JRE**：至少做“检测缺失 → 引导安装/下载 Temurin”的闭环。
5. **实例与配置**：在现有 JSON 配置上增加每实例 game dir/Java/内存，脱离“版本即实例”。
6. **下载中心后续**：CurseForge 品类 id 映射（现在选品类只查 Modrinth）、整合包/世界浏览、按加载器/版本筛选项。
7. **发布工程**：版本号、CHANGELOG、SHA256、签名、自动更新。

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

### 6.1 测试卫生（血泪教训）

手工联调/冒烟测试的 game dir **一律**放 `target/scratch/<name>` 或 `/tmp/nailong-<name>`，禁止落在 `$HOME` 下。

- 每次安装都会拉 libraries/assets，单个目录 250 MB 起步；过去在 `$HOME` 下留过 17 个 `nailong-*` 测试目录，清掉后约 1.1 GB。
- 不给 `--root`/game dir 直接跑 GUI 时，默认数据目录是 `$XDG_DATA_HOME/nailong/.minecraft`（即 `~/.local/share/nailong`），同样会累积完整实例。
- 这些目录不在仓库里，`git status` 永远是干净的，所以**收尾时要用 `find ~ -maxdepth 2 -iname 'nailong*'` 单独查一遍**。
