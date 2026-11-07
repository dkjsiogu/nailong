# 23 · 界面代码的框架

> 0.38 之前，界面的全部代码在一个 12000 多行的 `main.rs` 里：155 个平铺的状态字段、所有页面、所有弹窗、所有后台消息和所有通用组件挤在一起，改一处要翻几千行。界面看起来是分层的，代码也应该是。这一页讲 0.38 的结构和写新功能时的规矩。

## 1. 一眼看懂的目录

```
crates/nailong-ui/src/
  main.rs        入口：模块清单和 main()，70 行
  prelude.rs     每个模块开头 use crate::prelude::*，常用的东西一次带齐
  app.rs         App：全部状态（按功能分组）和每一帧的流程
  msg.rs         后台线程回到界面的唯一入口：Msg 和 poll()
  activity.rs    后台任务给人看的样子：步骤、刚下载的文件、来源、速度、剩余时间
  model.rs       数据类型：版本行、导航、任务种类、这台电脑（Env）……
  play.rs        开始游戏：合并启动参数、选 Java、算 JVM 参数、缺 Java 时下载
  widgets.rs     通用组件：按钮、胶囊、输入框、搜索框、路径行、标签……
  fonts.rs       字体和窗口图标
  theme.rs  motion.rs  rings.rs    配色、动效、状态圆环
  pages/         四个一级页面和进度页
    mod.rs         导航和侧边栏
    home.rs        启动台          shots.rs  截图的底座：列表、缩略图、启动台那一排、看大图
    gallery.rs     截图页：总览、时间、地点、人物
    versions.rs    版本            progress.rs 安装和下载进行中
    worlds.rs      存档
    content/       内容：mod.rs 页头和共用部分，browse 发现，detail 项目详情，
                   library 已安装，drop 拖放导入
  dialogs/       盖在页面上的弹窗
    settings.rs  account.rs（账户列表、微软登录、头像）  add_account.rs（添加账户、外置登录）
    params.rs（启动参数）  log.rs  rollback.rs
    review.rs（安装确认）  update.rs（启动器更新）
```

最大的文件 1300 行（版本页），其余多在 300–900 行；`main.rs` 从 12039 行变成 70 行。

## 2. 状态按功能分组

`App` 不再平铺 155 个字段，而是分成五段，每段写清楚是什么：

- **数据和选择**：数据目录、版本列表、当前选择、账户、偏好（`prefs`，存在 `ui.json`）。
- **导航**：哪个页面在前、切页动效。
- **各页面和弹窗自己的状态**：`home`、`install`、`content`、`worlds`、`login`、`rollback`、`update`……每组是一个结构体，定义在管它的那个模块里（例如 `ContentState` 在 `pages/content/mod.rs`）。
- **开始游戏和这台电脑**：`play`（每个版本的启动参数、调参缓存、最近一次启动）和 `env`（找到的 Java、电脑配置、测试结论）。
- **后台任务和提示**：正在跑的任务、它的 `activity`、页面底部的提示和记录。

代码里读起来是 `self.content.query`、`self.worlds.backup`、`self.env.javas`，一看就知道属于哪一块。

## 3. 一帧怎么走

`app.rs` 的 `ui()` 从上到下：

1. `poll()` 收后台结果，`poll_update()` 收更新检查的结果；
2. 快捷键、图标、自动备份、拖放；
3. 侧边栏，然后当前页面（有整页任务时是进度页）；
4. 弹窗一层层画上去：账户、设置、测试确认、恢复点、安装确认、日志、启动参数、更新、截图预览。

后台线程只做两件事：往外报 `Progress`，结束时发一条 `Msg`。界面状态只在主线程改，所以不需要锁。

## 4. 写新功能的规矩

- **一个功能一个文件**。新页面放 `pages/`，新弹窗放 `dialogs/`，在文件里写 `impl App { ... }` 和它自己的状态结构体。
- **状态跟着功能走**。新功能的字段放进自己的状态结构体，在 `App` 里占一行，并写一句说明。
- **通用的东西进 `widgets`**。两个以上页面会用的组件放 `widgets.rs`，`prelude` 会自动带上；只有一个页面用的就留在那个页面里。
- **后台任务走 `Activity`**。长任务把 `Progress` 报出来（带上 `current` 文件名和 `source` 主机），界面就自动有步骤、速度、剩余时间和「刚下载」。
- **模块名不和 core 撞**。core 已有 `installed`、`saves`、`launch`，界面里对应的叫 `library`、`worlds`、`params`。
- 手感按 [21](21-handling.md) 自查，视觉按 [18](18-ui-fog.md)。

## 5. 这次顺手变强的地方

- **下载页**：以前只有百分比和文件数；现在能看到做完了哪几步、各用了多久，正在做哪一步，刚下载的几个文件（越早的越淡），来自 Mojang 官方还是 BMCLAPI 镜像，速度和剩余时间。资源文件显示真名（`minecraft/sounds/...`），不是哈希。
- **Java 自动下载**：用官方启动器同一份运行时清单，优先下 LZMA 压缩版（约三分之一大小），逐个校验 SHA-1，下到 `.part` 目录、全部通过后才改名生效。点「开始游戏」时没有合适的 Java 就直接下载，下完接着启动；设置、首次引导和启动参数里也能手动下。
- **启动器更新**：启动时安静地查一次（最多 12 小时一次），有新版本时侧栏出现小提示；更新说明来自发版提交的说明；下载后按 SHA256SUMS 校验，Windows 一键重启到新版本，Linux 给出能复制的安装命令。
