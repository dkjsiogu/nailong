# nailong

轻量的 Minecraft 启动器，使用 Rust 编写。

> 当前版本：**0.1.0 · NeoForge 预览版**
> 目标：单文件、无额外运行时、低内存、快速启动；先跑通一条完整路径，再扩展。

## 下载

前往 [Releases](https://github.com/dkjsiogu/nailong/releases/latest) 下载：

| 平台 | 文件 | 说明 |
|---|---|---|
| Windows x64 | [nailong.exe](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong.exe) | **单个 exe，不用解压**，双击即用 |
| Linux x64 (deb) | [nailong_0.1.0_amd64.deb](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong_0.1.0_amd64.deb) | 本机 Ubuntu 26.04 amd64 构建 |

校验值：[SHA256SUMS](https://github.com/dkjsiogu/nailong/releases/latest/download/SHA256SUMS)。

### Windows

1. 下载 `nailong.exe`，放到任意目录，双击运行；
2. 第一次使用：展开“安装 NeoForge”，保持默认 `1.21.1 / 21.1.209`，点“开始安装”；
3. 安装完成后选中实例，填用户名，点“启动”。

需要系统里有 **Java 21**。启动器会自动查找；没找到会在状态栏提示。

> exe 未做代码签名，Windows SmartScreen 可能提示“未知发布者”；选择“更多信息 → 仍要运行”即可。杀毒软件误报也偶有发生。

### Linux（Debian / Ubuntu）

```bash
sudo apt install ./nailong_0.1.0_amd64.deb
nailong
```

也可以双击 deb 用软件中心安装。安装后开始菜单里会有 **nailong**。

卸载：

```bash
sudo apt remove nailong
```

> 这个 deb 是 **Ubuntu 26.04 / amd64** 上构建的，要求 glibc ≥ 2.43；Ubuntu 24.04 等旧系统暂时装不上。需要的话可以按下面的“从源码构建”自己打一个。

数据目录默认：

- Windows：`%APPDATA%\nailong\.minecraft`
- Linux：`~/.local/share/nailong/.minecraft`

## 现在能做什么

- **安装 Minecraft 原版**（已验证 1.21.1）
- **安装 NeoForge**（已验证 1.21.1 / 21.1.209）
- **离线账户启动**（单人 / 局域网）
- **图形界面**：选实例、填用户名/内存、安装 NeoForge、一键启动
- **命令行**：安装、启动、查看 Java、打印启动计划
- **核心单元测试**：版本规则、Maven 坐标、natives、离线 UUID（`cargo test --workspace`）
- **Windows + Linux**：Windows 为单 exe；Linux 提供 deb，安装后可直接从应用菜单启动

| 产物 | 大小 | 说明 |
|---|---:|---|
| Windows GUI `nailong.exe` | 10.5 MB | 单文件，无 MinGW 运行库依赖 |
| Linux GUI（deb 内） | 11.6 MB | 单二进制 |
| `nailong_0.1.0_amd64.deb` | 4.2 MB | 含桌面图标、菜单项、GPL 许可 |

> 实机验证：Linux（Xvfb + Mesa）安装 NeoForge 1.21.1 并启动到主菜单；Windows 已交叉编译并检查依赖，但尚未在 Windows 实机启动，欢迎帮忙试。

## 命令行（可选）

deb 只装了 GUI；如果你自己构建，会同时得到 CLI：

```bash
./target/release/nailong --root /path/to/.minecraft list
./target/release/nailong --root /path/to/.minecraft install neoforge 1.21.1 21.1.209
./target/release/nailong --root /path/to/.minecraft launch neoforge-21.1.209 --username Steve --memory 4096
./target/release/nailong --root /path/to/.minecraft launch neoforge-21.1.209 --print-plan --dry-run
```

## 从源码构建

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
- Fabric / Forge / 整合包（下一步）
- 自动更新、模组管理、皮肤
- 图形界面的安装进度条（目前只有状态文字；CLI 会打印阶段）
- NeoForge 安装内置化：当前复用 **NeoForge 官方安装器**做一次性安装，启动器本体不依赖它；下一步换成内置 processors

## 代码结构

```text
crates/
  nailong-core/   版本解析、下载、Java 检测、启动、NeoForge 安装
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
