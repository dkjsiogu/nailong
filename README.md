# nailong

[![CI](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml/badge.svg)](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml)

轻量的 Minecraft 启动器，使用 Rust 编写。

> 当前版本：**0.4.3 · 四加载器预览版**
> 目标：单文件、无额外运行时、低内存、快速启动；版本可选、安装可见、中断可补。

## 下载

前往 [Releases](https://github.com/dkjsiogu/nailong/releases/latest) 下载：

| 平台 | 文件 | 说明 |
|---|---|---|
| Windows x64 | [nailong.exe](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong.exe) | **单个 exe，不用解压**，双击即用 |
| Linux x64 (deb) | [nailong_0.4.3_amd64.deb](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong_0.4.3_amd64.deb) | amd64，兼容 Ubuntu 24.04+ |

校验值：[SHA256SUMS](https://github.com/dkjsiogu/nailong/releases/latest/download/SHA256SUMS)。

### Windows

1. 下载 `nailong.exe`，放到任意目录，双击运行；
2. 第一次打开就是安装页：选 Minecraft 版本，再在 Fabric / NeoForge / Forge / 原版中选一个，点“安装”；
3. 进度条走完后会自动回到主页，点“启动游戏”。

需要系统里有 **Java 21**。启动器会自动查找；没找到时界面上会提示。

> exe 未做代码签名，Windows SmartScreen 可能提示“未知发布者”；选择“更多信息 → 仍要运行”即可。杀毒软件误报也偶有发生。

### Linux（Debian / Ubuntu）

```bash
sudo apt install ./nailong_0.4.3_amd64.deb
nailong
```

也可以双击 deb 用软件中心安装。安装后开始菜单里会有 **nailong**。

卸载：

```bash
sudo apt remove nailong
```

> 这个 deb 在 **Ubuntu 26.04 / amd64** 上构建，已做兼容处理，要求 glibc ≥ 2.39；Ubuntu 24.04+ 可以直接安装，22.04 等更旧的系统暂时装不上。

数据目录默认：

- Windows：`nailong.exe` 同目录下的 `.minecraft`（便携）
- Linux：`~/.local/share/nailong/.minecraft`

## 现在能做什么

- **安装 Minecraft 原版**（已验证 1.21.1）
- **安装 Fabric / NeoForge / Forge**（已验证 1.21.1：Fabric 0.19.5、NeoForge 21.1.209、Forge 52.1.20）
- **Forge / NeoForge 原生安装**：Rust 读取安装描述并逐个执行客户端 processor，不再调用官方安装器
- **安装数据按需读取**：只取 `install_profile`、`version.json` 和客户端补丁（约 3 MB），不再下载 6–9 MB 的完整安装器；补丁分 4 段并行读取，镜像不支持分段时自动回退
- **重复安装秒完成**：已完整安装的 Forge / NeoForge 直接复用，不再下载补丁、不再重跑 processor
- **四种安装方式**：安装页按 Minecraft → Fabric → NeoForge → Forge → 原版排列，版本全部下拉选择
- **Modrinth 整合包**：拖入或选择 `.mrpack`，自动装加载器、模组和覆盖文件，生成独立游戏目录
- **离线账户启动**（单人 / 局域网）
- **版本选择**：Minecraft 和三种加载器都是下拉框，自动从官方清单读取，不用手输
- **版本管理**：加载器依赖的原版自动隐藏，不再和 NeoForge 并排显示；管理页可删除版本/整合包，被依赖的版本会阻止误删
- **安装进度**：阶段、百分比、已下载 / 总大小、文件数
- **中断可补**：启动时检查缺哪些文件；缺资源会显示“补全资源”，修完再启动
- **图形界面**：一屏完成选版本、安装、启动；玩家名/内存收在下方一行
- **命令行**：安装、启动、查看 Java、打印启动计划
- **Windows + Linux**：Windows 为单 exe；Linux 提供 deb，安装后可直接从应用菜单启动

| 产物 | 大小 | 说明 |
|---|---:|---|
| Windows GUI `nailong.exe` | 约 12 MB | 单文件，无额外运行库 |
| Linux GUI（deb 内） | 约 11.6 MB | 单二进制 |
| `nailong_0.4.3_amd64.deb` | 约 4.3 MB | 含桌面图标、菜单项、GPL 许可 |

> 实机验证：Linux（Xvfb + Mesa）下 Fabric / NeoForge / Forge 1.21.1 均启动到游戏渲染/主菜单；Forge / NeoForge 只缓存约 3 MB 的精简安装数据，完整安装器不再落盘；原版基础版本已从主列表隐藏，管理页可删除；重复安装走快速路径；导入 Fabulously Optimized `.mrpack` 后加载 151 个模组并启动；删除资源后会提示缺失并可补全。Windows 由 CI 原生编译，尚未在 Windows 实机启动，欢迎帮忙试。

## 命令行（可选）

deb 只装了 GUI；如果你自己构建，会同时得到 CLI：

```bash
./target/release/nailong --root /path/to/.minecraft list
./target/release/nailong --root /path/to/.minecraft install neoforge 1.21.1 21.1.209
./target/release/nailong --root /path/to/.minecraft install modpack ~/Downloads/pack.mrpack
./target/release/nailong --root /path/to/.minecraft launch neoforge-21.1.209 --username Steve --memory 4096
```

## 构建

发布产物由 GitHub Actions 构建，不占本机性能：

- **CI**：`fmt + clippy + test`，每次 push / PR 自动跑
- **Release**：手动运行 `Release` workflow（或 push `v*` tag），自动构建 Windows exe 和 Linux deb，并上传到对应 Release

本地构建（Rust 1.95+）：


```bash
# GUI + CLI
cargo build --release -p nailong-ui -p nailong-cli

# Linux: target/release/nailong-ui 是 GUI，target/release/nailong 是 CLI
# Windows (GNU 交叉编译示例): --target x86_64-pc-windows-gnu
```

打 deb：

```bash
./scripts/build-deb.sh
# 产物: dist/nailong_<version>_<arch>.deb
```

## 这一版刻意没做

- 微软正版登录（client id / 审批未就绪；先离线）
- CurseForge / Prism 整合包（目前支持 Modrinth .mrpack）
- 自动更新、模组管理、皮肤
- 安装取消按钮（关闭窗口可中断；下次启动会检测并补全）

## 代码结构

```text
crates/
  nailong-core/   版本解析、下载、Java 检测、启动、Fabric/NeoForge/Forge 安装
  nailong-cli/    命令行
  nailong-ui/     egui 图形界面
packaging/linux/  desktop、图标、AppStream 元数据、版权文件
scripts/          deb 打包脚本
docs/
  design/         架构与设计文档
  brief/          给老板看的 9 页简报
```

详细设计见 [设计文档索引](docs/design/00-overview.md)；当前实现进度与已知偏差见 [实现状态](docs/design/14-implementation-status.md)；给非技术读者的版本见 [9 页简报](docs/brief/nailong-brief.pdf)。

## 许可

GPL-3.0-or-later，见 [LICENSE](LICENSE)。

本项目与 Mojang Studios、Microsoft 无隶属关系；不附带、不分发任何 Minecraft 游戏资源。
