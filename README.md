# nailong

[![CI](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml/badge.svg)](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dkjsiogu/nailong?label=release)](https://github.com/dkjsiogu/nailong/releases/latest)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

轻量、原生的 Minecraft: Java Edition 启动器，使用 Rust 编写。Windows 只有一个 exe，Linux 只有一个 deb；不捆绑 Java 运行时，Windows 端也不需要安装器。

**简体中文** | [English](README.en.md)

> 当前版本：**0.32.0**（预览版）
>
> 目标：小体积、少依赖、安装过程可见、中断可补。功能边界见 [实现状态](docs/design/14-implementation-status.md)。

## 为什么是 nailong

装 Minecraft 本身不难，难的是装完以后干干净净：版本 JSON、资源索引、加载器 processor、Java 参数、整合包里的几百个文件，任何一环出错，玩家看到的都只是“启动失败”。

nailong 的思路很直：

- Rust 编写，启动器自身不运行在 JVM 上；Windows 是单文件，Linux 是一个 deb。
- 安装过程拆成可见的阶段；中途断了，重新打开会检查缺什么再补，不用从头再来。
- Forge / NeoForge 直接执行客户端 processor，不调用官方 installer，也不会把完整安装器留在磁盘上。
- 整合包拖进来就能识别，装加载器、下文件、解压覆盖一步到位。

它不是“万能启动器”，也还不是 1.0。能做和不能做的，下文列清楚了。

## 界面

0.32 把 0.31 的极光玻璃整体推倒，换成 Minecraft 原生的「深岩」像素语言：内置 Fusion Pixel 12px 中文字体，零圆角、2px 硬边、方块浮雕按钮、像素图标和深板岩砖背景；启动台主视觉是一块在 Rust 里逐块拼出的等距体素岛（草方块、树、水池、矿脉、站在边缘的小人、四层石/土裙边、悬浮金块）。所有页面与弹窗共用同一套方块组件；1120 宽恢复 4 列内容网格，900×620 隐藏最近世界槽和状态条、设置正文可滚动。背景按尺寸生成并缓存为纹理，体素岛以整数像素多边形直接绘制；空闲 CPU 仍为 0.0%。

0.31 把整个界面换成了「极光 / 龙脊夜空」：深墨蓝底、衬线大标题、玻璃卡片；启动台变成一张完整的海报——左边是当前版本与「开始游戏」，右边浮着最近世界，下面接最近内容与快捷操作。背景和英雄区不是素材图，而是在 Rust 里按尺寸生成一次、缓存成纹理（极光、星场、月亮、龙形星座、方块山脊），所以画面更丰富，空闲时仍然不重绘；0.30 的空闲 0% CPU 保留。1120 宽恢复 4 列内容网格；设置弹窗在 900×620 下改为可滚动，内容和「完成」按钮不再被裁切。

0.30 把环境光从“每 66 ms 重绘整窗”改成静态：渐变、点阵、微粒都还在，但只在切页、悬停、进度更新时出帧。Xvfb + llvmpipe、窗口聚焦、空闲 5 秒实测，单进程从约 162% CPU 降到 0%。这也是之前“首页/切页烫”的根因。

0.29 加「减少动效」：设置里一个开关，写入 `ui.json`。开启后页面淡入直接完成、环境光粒子静止、卡片悬停不抬升、按钮不位移、进度条扫光关闭、开关立即到位；空闲时也不再为环境光重绘。颜色和描边反馈保留，不会“关掉动效就变呆”。

0.28 补键盘路径：Ctrl+1–4 切启动台/版本/内容/存档，Ctrl+, 打开设置，F5 刷新版本列表；输入框聚焦或有弹窗时不抢键。快捷键提示放在设置弹窗诊断区下方，不占主界面。

0.27 收设置弹窗：内存从裸输入框换成 `− 4 GB +` 的 1 GB 步进器；Java 行显示实际检测版本，可「重新检测」，缺失时直接给「下载 Java 21」；玩家名、内存、Java 拆成两行，不再挤在一条线上。

0.26 给更新加确认：「全部更新」和单行「更新」不再直接覆盖文件，而是先弹确认清单——每个项目从哪个版本到哪个版本、目标版本、写入目录、项目数和下载大小，并说明旧文件会清理、更新前自动创建恢复点。更新和安装现在共用同一套“先看一眼再写盘”的语言。

0.25 继续收页面里的控件：版本页统计改成大数字在上、标签在下；自动备份和「显示基础版本」不再用默认 checkbox，换成内容页同一套暖色开关；存档工具条按“自动备份 → 间隔 → 保留份数 → 备份全部 → 清理”排顺。没有加功能，只把已有的页面读起来更顺。

0.24 把失败和关闭也当成界面：登录失败弹独立的「登录没有完成」结果卡，给「重新登录 / 关闭」，不再把一句 AADSTS 丢在首页背景里；设置、账户、恢复点、登录等待、登录失败、安装确认都支持 Esc 关闭；内容错误不再显示带完整 URL 的裸错误：没有内容时给「内容没有加载出来」居中卡，已有列表刷新失败只在顶部给重载条。原始错误只进设置日志。

0.23 先修玩家第一眼就会碰到的毛刺：Windows 打开登录链接不再经过 `cmd`，避免 URL 里的 `&` 被当成命令分隔符、登录参数被截断而报 AADSTS900144；登录窗口的“取消”不再污染内容浏览的取消状态，“发现”不会莫名其妙显示“操作已取消”。启动台/版本页的统计改成按页缓存，切页从 260 ms 收到 180 ms；内容首次加载换成带骨架的加载卡；1120 宽下发现/搜索结果从 3 列变 4 列。登录失败只显示一句能行动的话，原始 AADSTS 进日志。

0.22 让恢复点也管加载器：内容写进版本目录时，版本 JSON 和补丁 jar 会跟 Mod/配置一起进恢复点；恢复时先删掉这两个文件再按清单还原，不会出现“退回原版但还留着旧补丁 jar”。重装或修复已有加载器前也会自动留一份。共享 `libraries/` 不复制，旧库仍在；世界存档继续走独立快照。自定义外部 `gameDir` 的实例只恢复内容。

0.21 把“装之前先看一眼”补上：卡片 `+` 和详情页「安装」都会先弹出确认清单——装到哪个版本、实际写哪个目录、版本/文件、预计大小、会一起装的前置，以及安装前会自动创建恢复点；确认后才下载，取消不写盘。首次启动不再直接进入安装表单，而是三步引导：**确认 Java → 确认游戏目录 → 安装第一个版本或导入整合包**。没有版本时，启动台就是这张引导页。

0.20 把“坏了能救”做进内容库：安装、更新、导入、删除 Mod/资源包/光影前会自动留一个**恢复点**；装坏了在内容页点「恢复点」就能回到之前，恢复前还会为当前状态再留一份。恢复点存 Mod、配置、资源包、光影和内容索引，按整文件 SHA-1 去重，默认保留最近 10 份，不含世界存档（世界有自己的快照）。

0.19 把启动台从“仪表盘”收成一个主行动：**当前版本、开始游戏、版本详情、最近世界、最近内容**。账户、玩家名、内存、Java、游戏目录、数据目录和日志全部收进侧边栏底部的「设置」，玩家名与内存会保存到数据目录，重启后还在。产品方向写在 [产品 brief](docs/product/00-brief.md)：**装得干净，坏了能救**。

0.18 把界面按玩家的日常动线重新排了一遍：左边常驻四个入口——**启动台、版本、内容、存档**。内容库把「发现」和「已安装」合成一页，版本页只负责启动和维护，存档升为一级页面并自带版本选择器。视觉换成奶油白底、白卡片、橙黄强调的浅色主题；0.18.1 又补了字体与控制细节：真粗体、哑光琥珀主按钮、带播放/下载字形，次按钮也有独立的描边、悬停和接触阴影。

![动效演示：侧边栏切换、内容库、卡片悬停与详情返回](docs/screenshots/motion.gif)

| 启动台 | 内容 · 发现 |
|---|---|
| ![启动台](docs/screenshots/01-home.png) | ![内容 · 发现](docs/screenshots/05-discover.png) |
| **内容 · 已安装** | **存档** |
| ![内容 · 已安装](docs/screenshots/04-library.png) | ![存档](docs/screenshots/08-saves.png) |

更多截图（版本、安装、搜索、详情、进度、首次启动、设置、账户与登录、登录失败、恢复点、安装确认、空内容库、空存档，以及 6 张 900×620 窄窗口）：[docs/screenshots](docs/screenshots)。动效约 16 秒，包含侧边栏切页、发现、详情返回与启动台。界面字体为 [Fusion Pixel Font](https://github.com/TakWolf/fusion-pixel-font)，SIL OFL 1.1；许可证随 `crates/nailong-ui/assets/OFL.txt` 分发。

## 特性

### 安装与启动

- 原版、Fabric、NeoForge、Forge 都支持，版本从官方清单读，不用手输。
- Forge / NeoForge 走原生安装：只取 `install_profile`、`version.json` 和客户端补丁（约 3 MB），完整安装器不落盘。
- 安装拆成阶段：版本 JSON 就绪后，资源、client/libraries、加载器补丁和 processor 并行推进。
- 大文件按 4 段 Range 下载，段文件可续传；主源失败自动切备用源。
- 本机补丁缓存；客户端和运行库按 SHA1 存 blob，硬链接到目标路径；已有文件只做轻量检查。
- 本机现成的 patched client 也能复用：自动扫官方 / PCL / HMCL / Prism / MultiMC / Modrinth 和相邻 `.minecraft`，SHA1 匹配（NeoForge 走 JAR 结构校验）后直接复用，跳过补丁和 processor。
- 已完整安装的 Forge / NeoForge 再装一次等于 no-op，不会重复下载、重跑 processor。
- 资源缺失时明确提示“补全资源”，修完再启动。
- 安装可取消：下载、解压和 processor 都会响应；已下载部分保留，重新开始会检查补全。

### 整合包

支持六种格式，自动识别，不需要先选格式：

| 格式 | 入口文件 | 说明 |
|---|---|---|
| Modrinth | `.mrpack` | 直链 + SHA1 |
| CurseForge | `manifest.json` 的 zip | 按 project/file ID 解析；官方构建内置 key，校验 SHA1 |
| Prism / MultiMC | `mmc-pack.json` | 读取组件和 overrides |
| MCBBS | `mcbbs.packmeta` | addons + files + overrides |
| HMCL | `modpack.json` + `minecraft/pack.json` | 内嵌 manifest 转实例 |
| ATLauncher | `instance.json` | 内嵌 manifest 转实例 |

把 `.mrpack` 或 `.zip` 拖进窗口即可开始导入。单个 mod `.jar` 目前不会被当成整合包处理。

### 内容库：发现与已安装

- 左侧「内容」是唯一的 Mod / 资源包 / 光影入口；页头的「已安装 / 发现」是同一个库的两种模式，不用在两个页面之间跳。
- 右上角先选「安装到：<已装版本>」：选定后浏览、搜索都锁定这个版本的 Minecraft 版本和加载器，`+` 直接下进该版本的 `mods/` / `resourcepacks/` / `shaderpacks/`，不会再下错地方。
- 不想装也能纯逛：选「只浏览」，版本和加载器自己挑；卡片仍可看详情，点 `+` 会提示先选安装目标。
- 打开内容页默认看已安装：数量、启用/禁用、筛选、检查更新、逐项/全部更新、导入文件都在这一屏；空的时候直接给「去发现」。
- 切到「发现」就是浏览页：标题、搜索、内容类型和「安装到」固定在顶部，内容区是一排品类胶囊（优化、科技、冒险、写实、奇幻…）加「热门」「最新」两排卡片。不用先搜索，也不用先想关键词。
- 每张卡片是一个项目：图标、名字、简介、下载量，右下角一个 `+` 直接装。点 `+` 会取当前 Minecraft/加载器下最新的兼容版本，自动带上必需的 Modrinth 前置，装完卡片变成「已装」。
- 点卡片其它位置进详情：左边是项目信息、版本信息和前置，右边是安装面板与版本记录；挑具体版本，看 `required` / `optional` / `incompatible` 前置；`required` 自动加入，`optional` 是勾选项（默认不勾），`incompatible` 会阻止下载。
- 每排右上角有「更多 ›」：进入该排的完整列表（每页 24 个），继续「加载更多」翻页。
- 品类胶囊来自 **CurseForge 的真实品类树**：启动器运行时拉 `/v1/categories`，翻译成中文（科技、魔法、冒险、世界…），CurseForge 独有的品类照它的名字显示；Modrinth 有而 CurseForge 没有的品类补在后面。
- 品类默认只铺前 12 个，其余收进「更多品类」；可按来源切换：全部 / Modrinth / CurseForge。
- 版本和加载器都可以不选：默认「全部版本 · 全部加载器」，纯逛也能用；想看某个版本就切过去，切回全部不影响已装内容。
- 卡片带一小段介绍（名称、简介、下载量），完整描述和作者信息在详情页。选 CurseForge 时列表就是它的目录，按它的品类和排序走；下载仍由启动器解析地址并校验，不走网页。
- 搜索框还在右上角，回车即搜；搜索结果同样是卡片，也可以翻页。
- 浏览和安装都锁定当前选中版本：Mod 跟随该版本的加载器，资源包/光影只跟随 Minecraft 版本。
- 下载到该版本的 `mods/`、`resourcepacks/` 或 `shaderpacks/`。普通版本首次下载会启用独立 `gameDir`（`versions/<id>/`），不会和其他版本互相污染；根目录旧存档不会自动迁移。
- 「已安装」模式就是内容管理：按名称筛选、开关启用、检查更新、逐项/全部更新、重复项目提醒；删除走 `⋯` 菜单里的行内二次确认，更新一个禁用项不会把它悄悄启用。
- **安装确认**：卡片 `+` 和详情安装都先看清单（目标版本、写入目录、版本/大小、前置、恢复点说明），确认才下载；取消不写盘。
- **恢复点**：安装、更新、导入、删除内容、重装加载器前自动创建；内容页「恢复点」可手动创建、恢复、删除。默认实例会连版本 JSON 和补丁 jar 一起保存；恢复前自动留一份当前状态，恢复时校验对象哈希。留点失败会中止操作，不静默继续。
- 支持本地 `.jar` 拖入/导入，以及资源包/光影的本地 `.zip` 导入。
- 同名文件内容相同则跳过，内容不同会报冲突，不静默覆盖；更新会替换旧版本并保留索引。

### 日常使用

- 微软正版账户登录：点左侧边栏底部的账户（离线时显示「点击登录」），走浏览器授权码 + PKCE（本地回环端口），成功后自动刷新令牌；启动时带 `--accessToken` / `--uuid` / `--xuid`。
- 令牌存在数据目录的 `account.json`（Unix 0600），不写日志；退出登录会删除。
- 离线账户启动（单人 / 局域网），没登录时就是原来的离线模式。
- 设置：侧边栏底部「设置」集中玩家名、内存、Java 状态、游戏/数据目录和日志；玩家名与内存写入 `ui.json`，下次打开还在。
- 版本：加载器依赖的原版自动隐藏；被依赖的版本不能误删；只负责启动、补全、打开目录和删除，内容与存档分别去左侧对应入口。
- 存档：一级页面，顶部可直接切换版本；列出世界、显示大小和最近修改时间；备份是 **git 式快照**——按内容分块（CDC）只存改动过的块，第二次备份通常只增加几 MB；可恢复、删除、按份数自动清理。
- 自动备份：可设每 15/30/60/120 分钟一次、每个世界保留 5/10/20/50 份；配置存数据目录的 `backups.json`，启动器开着就会按点跑。
- 资源包/光影：和 Mod 一样可以启用/停用、删除、看文件大小、一键打开所在目录。
- 安装进度：阶段、百分比、已下载 / 总大小、文件数。
- GUI 和 CLI 共用同一套 core；CLI 适合脚本和排障。

## 下载

| 平台 | 文件 | 大小 | 要求 |
|---|---|---:|---|
| Windows x64 | [nailong.exe](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong.exe) | 约 16 MB | Windows 10/11，Java 21 |
| Linux x64 (deb) | [nailong_0.32.0_amd64.deb](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong_0.32.0_amd64.deb) | 约 4.8 MB | Ubuntu 24.04+ / glibc ≥ 2.39，Java 21 |

校验值：[SHA256SUMS](https://github.com/dkjsiogu/nailong/releases/latest/download/SHA256SUMS)。

### Windows

1. 下载 `nailong.exe`，放到任意目录，双击运行。
2. 第一次打开是三步引导：确认 Java、确认目录，再点“安装第一个版本”；选 Minecraft 版本和 Fabric / NeoForge / Forge / 原版，点“安装”。
3. 进度走完后回到主页，点“启动游戏”。

需要系统里有 **Java 21**。启动器会自动查找；没找到时界面会提示。

> exe 没有代码签名，Windows SmartScreen 可能提示“未知发布者”。选择“更多信息 → 仍要运行”即可。杀毒软件误报也偶有发生。

### Linux（Debian / Ubuntu）

```bash
sudo apt install ./nailong_0.32.0_amd64.deb
nailong
```

也可以双击 deb 用软件中心安装。安装后应用菜单里会有 **nailong**。

卸载：

```bash
sudo apt remove nailong
```

> deb 在 Ubuntu 26.04 / amd64 上构建，要求 glibc ≥ 2.39；Ubuntu 24.04+ 可以直接安装，22.04 等更旧的系统暂时装不上。

### 数据目录

- Windows：`nailong.exe` 同目录下的 `.minecraft`（便携）。
- Linux：`~/.local/share/nailong/.minecraft`。

可以用 `--root` 指定其他目录。

## CurseForge API（内置）

官方 Release 已内置项目 key，导入 CurseForge 整合包时自动走官方 API，**普通用户无需设置**。这不是 CurseForge 账号密码，而是应用级 key。

- 官方 API 返回文件名、大小、SHA1 和下载地址，下载后可以校验。
- 作者关闭第三方分发：停止导入并列出文件，不会手拼 CDN 链接绕过。
- 维护者或调试时可以用环境变量 `CURSEFORGE_API_KEY` 覆盖内置值，下一版换 key 不需要改代码。
- 内置 key 做了简单的 XOR 混淆，避免 `strings` 直接看到；**这不是加密**，仍然可以从二进制里还原，只作为共享默认配额，不能当秘密。
- 如果你自己构建且没有注入 key，仍然可以导入，只是走 cfwidget + 官方下载页回退，没有官方 SHA1。

## 验证范围

已经实际跑过的：

- GUI：1120×720 与最小 900×620 下逐页走查，覆盖侧边栏四入口、首次启动引导、安装确认、内容发现/已安装、搜索、详情、空状态、存档快照、设置弹窗、内容恢复点、进度页和账户/登录弹窗；24 张截图（18 张 1120×720 + 6 张 900×620）和 16 秒动效 GIF 在 `docs/screenshots/`。0.32 复测窗口聚焦、鼠标移开后的空闲 CPU 为 0.0%。
- Linux（Xvfb + Mesa）：Fabric 0.19.5、NeoForge 21.1.209、Forge 52.1.20 在 1.21.1 启动到主菜单。
- Fabulously Optimized `.mrpack`：151 个模组加载并启动。
- CurseForge：真实文件通过无 key 的官方下载页回退下载成功；官方 API key 实测返回文件名、大小、SHA1 和下载地址。
- GUI 取消安装：Xvfb 下安装中点取消，回到“继续安装”页，进程未崩溃。
- 内容库：真实 Modrinth + CurseForge API；Mod/资源包/光影检索；Jade 及必需前置下载到版本独立 `mods/`；已安装扫描/启停/重复检测/更新检查；0.10.0 实测浏览页卡片 `+` 一键装 Fabric API 到版本独立 `mods/` 并写入内容索引。
- Prism、MCBBS、HMCL、ATLauncher：真实或合成样例包跑通格式识别与实例创建。

还没验证的：

- Windows 由 CI 原生编译，但尚未在 Windows 实机启动。
- 微软登录的令牌存储还是 0600 文件，没有接系统钥匙串（Windows Credential Manager / Secret Service）。
- macOS 未支持，也没有验证。

## 命令行

deb 只装 GUI；自己构建会同时得到 CLI：

```bash
./target/release/nailong --root /path/to/.minecraft list
./target/release/nailong --root /path/to/.minecraft install neoforge 1.21.1 21.1.209
./target/release/nailong --root /path/to/.minecraft install modpack ~/Downloads/pack.mrpack
./target/release/nailong --root /path/to/.minecraft install modpack ~/Downloads/pack.zip
./target/release/nailong --root /path/to/.minecraft launch neoforge-21.1.209 --username Steve --memory 4096
```

## 从源码构建

需要 Rust 1.95+。Linux 还需要编译 egui 的系统依赖：

```bash
sudo apt install build-essential pkg-config \
  libgl1-mesa-dev libegl1-mesa-dev \
  libx11-dev libxext-dev libxcursor-dev libxi-dev libxrandr-dev \
  libxkbcommon-dev libxkbcommon-x11-dev
```

构建 GUI + CLI：

```bash
cargo build --release -p nailong-ui -p nailong-cli
```

Linux 构建 deb：

```bash
./scripts/build-deb.sh
# 产物: dist/nailong_<version>_<arch>.deb
```

Windows 发布版由 GitHub Actions 使用 MSVC + 静态 CRT 构建；也可以用 `x86_64-pc-windows-gnu` 交叉编译做本地检查。

> 自己从源码构建时，如果没有设置 `NAILONG_CURSEFORGE_API_KEY`，构建出的程序会**禁用 CurseForge 源**（Modrinth 不受影响），这是有意的：key 在构建期混淆进二进制，不读配置文件。
>
> ```bash
> NAILONG_CURSEFORGE_API_KEY=... cargo build --release -p nailong-ui
> ```
>
> 想验证真实接口，跑被忽略的联网冒烟测试：
>
> ```bash
> cargo test -p nailong-core --test live_modrinth -- --ignored --nocapture
> ```

## 还没做

- 启动器自身自动更新、皮肤、完整整合包更新。
- 启动后的停止游戏 / 启动取消。
- Windows 实机验证、macOS 支持。


## 代码结构

```text
crates/
  nailong-core/   版本解析、下载、Java 检测、启动、加载器安装、整合包导入、内容库
  nailong-cli/    命令行
  nailong-ui/     egui 图形界面
packaging/linux/  desktop、图标、AppStream 元数据、版权文件
scripts/          deb 打包脚本
docs/
  design/         架构与设计文档
  brief/          9 页项目简报
```

详细设计见 [设计文档索引](docs/design/00-overview.md)；实现进度与已知偏差见 [实现状态](docs/design/14-implementation-status.md)。

## 许可

GPL-3.0-or-later，见 [LICENSE](LICENSE)。

本项目与 Mojang Studios、Microsoft 无隶属关系；不附带、不分发任何 Minecraft 游戏资源。
