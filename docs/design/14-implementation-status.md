# 14 · 原型实现状态（0.1 → 0.39.0）

> 日期：2026-11-07
> 对应发布：GitHub Release `v0.39.0` 的 `cairn.exe`、`cairn_0.39.0_amd64.deb`（0.37 起改名 Cairn）
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

0.33.0 同时理顺了运行中的流程：游戏运行时不再锁住整个启动器，只锁正在使用的版本的写操作；开启自动备份后每次退出游戏都会留一份快照；启动前先检查 Java；版本、内容、存档页底部统一显示错误和“查看日志”入口。界面按先定稿、后实现的流程把界面重做成「雾蓝」风格（规范见 `docs/design/18-ui-fog.md`）：`theme.rs` 集中管理深浅两套颜色并由测试检查对比度，设置里可选跟随系统、深色或浅色；`rings.rs` 实现启动台的状态圆环，显示文件完整度、每个 Mod 和快照，支持悬停说明、点击跳转和键盘操作。删除 `voxel.rs`、`backdrop.rs` 和内置 Fusion Pixel 字体，中文改用系统字体，大字用 egui 自带的 Ubuntu Light。Xvfb + llvmpipe 聚焦空闲复测 0.0% CPU；真实 GPU 和 Windows 待测。

0.34.0 加入 JVM 自动调参（`tuning.rs`，规范见 `docs/design/19-jvm-tuning.md`）：内存默认自动，按物理内存、逻辑处理器、Java 版本、加载器和 Mod 数量算出堆大小，配官方启动器那套 G1 参数并按堆放大 Region；Java 25 加紧凑对象头；大整合包配强机器时自动换分代 ZGC。设置里显示结果、依据和完整参数，可手动指定内存、回收器或追加 JVM 参数；CLI 增加 `--gc`、`--jvm-arg`，`--dry-run` 打印计算过程。生成的参数在本机 OpenJDK 11/17/21/25 和 GraalVM 21 上逐一启动验证。顺带修复 CLI 传相对 `--game-dir` 时游戏目录被解析两次的问题。

0.34.1 修复 26.x 无法安装和启动：版本文件 `default-user-jvm` 的首条没有 `rules`，解析失败；`${natives_directory}` 误指向 `java` 子目录，`java.library.path` 变成 `.../java/java`。实测 26.1、26.3 原版可安装并进入世界。调参在版本文件带官方默认时跟随官方（26.x 为 ZGC + 紧凑对象头 + 预先占用 + 字符串去重，初始堆 2 GB）。Java 选择改为够用里最低的大版本，同版本优先 64 位 OpenJDK。

0.39.0 公共镜像（见 `docs/design/24-download-sources.md`）。core：`http::fallback_urls` 给每个地址排好候选——Mojang 和加载器官方 ↔ BMCLAPI 互为后备（「镜像优先」时 BMCLAPI 在前），Modrinth / CurseForge 接口总是官方在前、MCIM（`mod.mcimirror.top`，UA 已在 mcim-rust-api#12 登记）在后，Mod 文件不走 MCIM（它的文件地址只是 302 回官方 CDN）；`get_text` / `get_json` / `get_bytes` / `get_json_with_headers` 统一走 `get_with`：每轮试一遍候选、官方的 400/404/410/422 直接作数、镜像的 404 等官方也连不上才作数；`x-api-key` / `Authorization` / `Cookie` 只发给原本的主机；官方连不上而后备成功时记五分钟（`fallback_for(host)`），期间同一主机先走后备，批量下载也一样；MCIM 回答里最旧的 `sync_at` 记下来给界面。`runtime.rs` 去掉自己的镜像重试。界面：设置里「下载源」（官方优先 / 镜像优先，存 `ui.json` 的 `source`，安装、整合包导入、版本目录和 Java 下载都按它走）和镜像致谢；内容页在用 MCIM 时显示提示和同步时间；下载页来源认得「MCIM 镜像」。实测：本地代理拦掉 `api.modrinth.com` 时搜索 0.8 秒从 MCIM 拿到结果，不拦时不碰 MCIM；拦掉 Mojang 全部五个主机时 `cairn-cli install vanilla 1.12.2` 53 秒装完。

0.38.0 下载体验和界面框架（见 `docs/design/23-ui-framework.md`）。core：`Progress` 多了 `current`（刚开始下载的文件，资源文件用资源索引里的真名）和 `source`（下载用的主机），`FileSpec.label` 给进度显示用；`runtime.rs` 按 Mojang 的运行时清单下载 Java（平台映射、按版本文件里的组件或够用里最低的大版本选择、清单 SHA-1 校验、优先 LZMA 压缩版并用 `lzma-rs` 解压、逐个 SHA-1、可执行权限和符号链接、`.part` 目录完成后改名），`java::find_all(root)` 一并找数据目录里的运行时；`update.rs` 查 GitHub 最新版本、按平台选文件、按 SHA256SUMS 校验、Windows 改名替换正在运行的程序并重启。CLI 加了 `java-install` 和 `update`。界面：`main.rs` 拆成 `app`、`msg`、`model`、`play`、`activity`、`widgets`、`fonts`、`pages/`（home、versions、worlds、progress、shots、content/{browse,detail,library,drop}）和 `dialogs/`（settings、account、params、log、rollback、review、update），统一 `use crate::prelude::*`；App 的状态从 155 个平铺字段收成按功能分组的结构体（content、worlds、login、rollback、install、home、play、env、prefs、update）；进度页改用 `Activity`（步骤和耗时、刚下载的文件、来源、速度、剩余时间）；缺 Java 时启动前自动下载、下完接着启动；侧栏更新提示和更新弹窗，Release 的说明改用发版提交的说明。实测：界面里真实下载 Java 17（11.8 MB/s，133 个文件），`cairn-cli java-install 21` 15 秒装好并能运行；重构前后各页面截图一致。

0.37.0 改名 Cairn 并按 Windows 实测反馈改了七处（目标见 `docs/product/goal-0.37.md`）。改名：GUI 二进制 `cairn`、CLI `cairn-cli`、deb 包 `cairn`（Replaces/Conflicts/Provides nailong）、桌面文件和 AppStream id `io.github.dkjsiogu.cairn`（`<replaces>` 旧 id）；Linux 默认数据目录 `~/.local/share/nailong` 首次启动时整体改名为 `~/.local/share/cairn`，改名失败就继续用旧目录；数据目录里的 `cache/nailong` 同样改名；Windows 仍是 exe 旁边的 `.minecraft`。0.37 发布后 GitHub 仓库也改名为 `dkjsiogu/cairn`（旧地址自动跳转）；crate 名和版本目录里的 `.nailong-*` 标记文件不变。exe：`crates/nailong-ui/build.rs` 用 winresource 写入 `packaging/windows/cairn.ico`（16–256 七种尺寸）和版本信息，CI 检查 ProductName；窗口和任务栏图标用同一张 PNG。eframe 补上 `clipboard` 特性：0.36 关掉默认特性时漏了它，复制只在进程内有效、输入框粘贴不了外部文字。启动参数：`launchcfg.rs` 按版本存 Java、内存、回收器、额外 JVM 参数、窗口大小（`launch-settings.json`，全部跟随全局时不写），`LaunchOptions.java` 指定 Java 并在版本不够时给出原因，`launch::preview` 不碰磁盘地生成命令，`Prepared::display_lines`/`command_line` 给界面显示和复制（令牌打码）；启动台在「开始游戏」下放参数胶囊，点开是按版本编辑的窗口和实时命令行预览。截图墙：`shots.rs` 读游戏目录的 `screenshots/`，后台解码、缩小到 320×180 并缓存到 `cache/cairn/thumbs/`，左键从缩略图放大成大图（←/→、Esc），右键或 Ctrl+C 把原图复制到剪贴板，动画只在进行中请求重绘。已安装内容：索引记下每个文件的大小和修改时间（纳秒），没变就不再打开压缩包；进页面先显示索引，后台核对后替换；启动台的 Mod 数也在后台核对。文字：启动台版本名按宽度缩小字号、太长两行并让出空间；路径行（设置、版本页）从中间省略、悬停全路径、点击复制。下载提示：实测 HMCL 3.16.4 的 exe 也没签名，区别在下载量；CI 预留 SignPath 签名任务（`vars.SIGNPATH_ORGANIZATION_ID` 为空时跳过），说明见 `docs/design/22-windows-trust.md`。实测：Xvfb 下深浅两套、1120×720 和 900×620 截图；右键复制后 X 剪贴板里是 1920×1080 的 image/png；交叉编译的 Windows exe 资源表里有 7 个 RT_ICON、1 个 RT_GROUP_ICON 和 RT_VERSION。

0.36.0 把调参拆成两半（规范见 `docs/design/20-tuning-benchmark.md`）。回收器：`bench.rs` 不再建测试实例，改为运行嵌入的 `assets/jvmbench/NailongBench.class`（Java 8 字节码，7 KB），模拟常驻数据、区块缓存、生成线程、服务端 tick 和不封顶渲染，对每个 64 位非 GraalVM 的 Java 21+ 分常规（存活 1 GB）和大型整合包（3 GB）两档各跑 G1、ZGC 一轮（预热 3–4 秒、测量 12 秒、`AlwaysPreTouch`），比平均帧率、1% 低帧、顿、tick P95、生成速度和 CPU，分别给出一般版本和官方默认 ZGC 版本的结论，存进 `jvm-bench.json`（线程数和内存变了就失效），所有版本共用。内存：`learned.rs` 每次启动加 `-Xlog:gc:file=logs/nailong-gc.log:uptime:filecount=0`（Java 9+、非 OpenJ9），退出后取启动 3 分钟后的老年代回收后的最大占用作存活数据，按版本 + 游戏目录存最近 5 局进 `learned.json`（Mod 指纹变了清空），整堆回收或晋升失败时下次至少加 1/4。GC 日志解析独立成 `gclog.rs`。界面：内容页状态胶囊筛选（全部/已启用/已禁用/可更新/重复）与搜索叠加、`Ctrl+F`、打开文件夹、在文件夹中显示、按内容识别的拖放导入（整批一个恢复点、跳过已装）和悬停提示、回到窗口自动重扫；新日志窗口（游戏输出/崩溃报告/启动器，原因识别 `launch::diagnose`，`Ctrl+L`）。实机：`nailong bench` 在 16 线程 16 GB、OpenJDK 21 和 25 上约 2 分 15 秒跑完，两次结论一致（常规档 G1、大型整合包档 ZGC，官方默认版本都沿用 ZGC），数字见 20 号文档；结论在读取时按当前规则重新判；真实启动 NeoForge 1.21.1 带 GC 日志参数正常、退出后记下一局；Xvfb 里用 GTK 拖拽源真实拖放 3 个文件，悬停提示和导入结果正确。

0.35.0 加入实测调优（`bench.rs`，规范见 `docs/design/20-tuning-benchmark.md`）：复制版本的 Mod、配置和资源包成测试实例，用内置的 1.20.1 / 26.1 世界模板（26.x 按目标 `world_version` 改写 NBT 数据版本，避免升级备份提示）和数据包，通过 `--quickPlaySingleplayer` 进入世界后以旁观模式每 tick 前进一格；从标准输出的标记计时，从 GC 日志和进程 CPU 时间算出存活数据、停顿和 CPU。三轮：低 IHOP 的 G1 测量、正式 G1 验证（整堆回收或晋升失败时加 1/4 重测）、ZGC 对比。结果按版本 + 游戏目录存进 `bench.json`，指纹含 Mod 列表、Java 大版本和内存，之后「自动」优先用实测值。`launch` 拆成 `prepare` 和启动两步供测试复用；CLI 的 `launch` 不写 `--game-dir` 时也按版本隔离选目录。Xvfb + llvmpipe 实测：NeoForge 1.21.1 存活约 0.5 GB → G1 2 GB（CLI 和界面各跑一次，结果一致）；26.3 原版 2 GB 出现 3 次晋升失败，加到 2.5 GB 后 G1 仍每分钟 6 次超过一帧，ZGC 3.5 GB 无停顿、CPU 不多 → ZGC；1.20.1 原版存活 1.0 GB → G1 2 GB（Java 17，不测 ZGC）。没有测帧率：原版无法把帧率写进日志。

0.32.0 把 0.31 的极光玻璃整体推倒，改成 Minecraft 原生的「深岩」像素语言：内置 Fusion Pixel 12px 中文字体（SIL OFL 1.1，7 MB），零圆角、2px 硬边、方块浮雕按钮、像素图标与 48×24 深板岩砖背景；启动台主视觉换成 Rust 逐块拼出的等距体素岛（草方块、树、水池、矿脉、小人、四层石/土裙边、悬浮金块），按尺寸缓存，空闲 CPU 仍为 0.0%。所有页面与弹窗统一方块组件；1120 宽 4 列内容网格，900×620 隐藏最近世界槽和状态条、设置正文滚动。新增 `docs/design/17-ui-deepslate.md`，24 张新截图和 16 秒动效替换 0.31 图。

0.31.0 把界面整体换成「极光 / 龙脊夜空」：深墨蓝玻璃、衬线大标题、程序化极光与星图、启动台英雄区和 bento 布局，同时保留 0.30 的空闲 0% CPU。`backdrop.rs` 在首次显示时按尺寸逐像素生成窗口背景和英雄区两张纹理（64×64 噪声、三条极光带、星场、月亮、龙形星座、三层方块山脊和树影），只缓存、不逐帧重绘；英雄区把左侧/底部文字遮罩和圆角直接烘进纹理。1120 宽内容网格恢复 4 列；设置弹窗在 900×620 下正文可滚动、底部按钮固定可见。新增 `docs/design/16-ui-aurora.md`，24 张新截图和 17 秒动效替换旧图；Xvfb + llvmpipe 聚焦空闲复测 0.0% CPU。

0.30.0 把环境光从“每 66 ms 重绘”改成静态：渐变、点阵、微粒纹理仍然在，但只在页面切换、悬停、进度更新等真正需要时出帧。Xvfb + llvmpipe、窗口聚焦、空闲 5 秒实测：改前单进程约 162% CPU，改后 0%。这是目前单项最大的一次性能收益，也解释了之前“首页/切页烫”的观感。减少动效仍负责关闭页面淡入、卡片抬升、按钮位移和进度扫光。

0.29.0 加「减少动效」：设置里一个开关，写入 `ui.json`。开启后页面淡入直接完成、首页环境光/浮动粒子静止、卡片悬停不再抬升、主/次按钮不再位移、进度条扫光关闭、开关滑块立即到位；空闲时也不再按 66 ms 请求环境光重绘。仍保留颜色/描边的即时反馈，避免“关掉动效后像坏了一样”。这是可访问性，也是性能预算的一部分。

0.28.0 补键盘路径：Ctrl+1–4 切换启动台/版本/内容/存档，Ctrl+, 打开设置，F5 刷新版本列表；输入框聚焦或有弹窗时不抢键。快捷键提示放在设置弹窗诊断区下方，不占主界面。Xvfb 实测 Ctrl+2 切到版本页、Ctrl+, 打开设置。

0.27.0 把设置弹窗里的默认控件换掉：内存从裸 `DragValue`（4096 MB）改成 1 GB 步进的 `− 4 GB +` 步进器；Java 行显示实际检测到的版本（如 Java 25.0.4.1），并提供「重新检测」；缺失 Java 时多一个「下载 Java 21」链接，不用退回首次启动页。玩家名/内存/Java 拆成两行，不再挤在一条水平线上。

0.26.0 给更新加确认：内容页「全部更新」和单行「更新」不再直接覆盖文件，而是先弹「确认更新」——列出每个项目从哪个版本到哪个版本、目标版本、写入目录、项目数和下载大小，并说明旧文件会清理、更新前自动创建恢复点。确认后才走原来的 `start_mods_update`；取消不写盘。这样“更新”不再是盲盒，和安装确认共用同一套清单语言。

0.25.0 统一版本页和存档页的控件层级：版本统计从“小标签在上、数字在下”改成 20 px 粗体数字在上、9.5 px 标签在下；自动备份和「显示基础版本」从默认 egui checkbox 换成内容页同一套暖色开关，开关状态和文字颜色一起变化；存档工具条按“自动备份 → 间隔 → 保留份数 → 备份全部 → 清理”从左到右排。没有新增功能，只把已经存在的页面读起来更顺。

0.24.0 把失败和关闭路径也当成界面的一部分：登录失败不再把一句错误丢在首页背景里，而是单独弹「登录没有完成」，给「重新登录 / 关闭」；原始 AADSTS 只进设置日志。Esc 现在能关闭设置、账户、恢复点、登录等待和登录失败弹窗；内容错误不再显示带完整 URL 的裸错误，网络失败时给「内容没有加载出来」居中卡和「重新加载」，已有列表刷新失败只在顶部给重载条。

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
| 并行下载、sha1 校验、重试、镜像后备（BMCLAPI、MCIM） | `core::http` | 完成 | 实际安装；官方被拦时自动换镜像 |
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
| 更新确认 | `nailong-ui` | 完成 | 全部更新/单行更新先展示 from → to、目标、项目数、大小与恢复点说明；确认才写盘；Xvfb 实测 3 个更新的确认弹窗 |
| 首次启动引导 | `nailong-ui` | 完成 | 无版本时进入三步引导：Java 21+ 检测/下载页/重新检测、目录展示、安装第一个版本或导入整合包；900×620 与 1120×720 复核 |
| GUI（侧边栏、版本、安装/补全、启动、内容库、存档、进度、账户） | `nailong-ui` | 可用 | Xvfb 下走完安装 / 补全 / 启动；安装中取消；四入口点击、内容发现/已安装切换、内容详情/搜索返回、存档版本切换实测；浏览页点卡片 `+` 实际下载并落索引；实际 NeoForge 到主菜单；0.23 复测登录取消后进发现不再误报取消，版本→启动台切换正常 |
| 一级信息架构 + 浅色主题 | `nailong-ui` | 完成 | 侧边栏 204/76 两档；内容合并发现/已安装；存档一级页 + 版本选择；版本页移除内容/存档入口；900×620 与 1120×720 截图复核 |
| 版本管理 | `nailong-ui` | 完成 | 基础版本隐藏、版本/整合包删除、被依赖保护 |
| CAS / patched client 复用 | `core::blob` / `core::install::reuse` | 完成（第一阶段） | client/libraries 新文件走 blob+硬链接；复用后跳过补丁与 processor；Forge 实测 5.3s、NeoForge 3.7s |
| 大文件分段下载 | `core::http` | 完成 | client jar 4 段并行、段续传、源回退 |
| 重复安装快速路径 | `core::install::forge` / `neoforge` | 完成 | health 通过则 0 下载、0 processor |
| Windows 交叉编译 | `x86_64-pc-windows-gnu` | 通过 | 无 MinGW DLL 依赖，仅系统 DLL |
| 发布包 | GitHub Release `v0.39.0` | 完成（CI 构建） | 单 exe 约 8 MB / deb 约 3.4 MB |

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
| 单元/集成测试体系 | **75 个测试**（66 core + 9 UI），另有 7 个真实网络冒烟测试默认忽略（搜索、下载、浏览、一键安装、无过滤浏览、设备码、回环探测） | 缺夹具、mock HTTP、启动 argv 快照、Windows CI |

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
