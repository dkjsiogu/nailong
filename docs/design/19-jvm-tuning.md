# 19 · JVM 自动调参

> 启动时按电脑配置、Java 版本和 Mod 数量算出堆大小和 GC 参数。代码在 `crates/nailong-core/src/tuning.rs`。

## 1. 原则

1. **只用验证过的参数。** JVM 遇到不认识的参数会直接拒绝启动，比不调更糟。实验参数只给已验证的 Java 版本，更新的 Java 只用正式参数。
2. **堆够用就好。** 堆越大，每次回收要扫的越多，停顿越长；还会挤占系统和显卡驱动的内存。
3. **用户说了算。** 手动内存、手动回收器、自定义参数都优先于自动结果。
4. **算法可见。** 设置里能看到算出来的值、依据和完整参数，`--dry-run` 打印同样的计算过程。

## 2. 堆大小

**需要多少：**

| 项 | 取值 |
|---|---|
| 基础 | Java 17 及以上（1.18+，世界更高、区块更大）3 GB；更老的版本 2 GB |
| 加载器 | Fabric/Quilt +256 MB；Forge/NeoForge +512 MB；原版 0 |
| 每个 Mod | Fabric/Quilt 10 MB；Forge/NeoForge 20 MB |

结果向上取整到 512 MB，限制在 2–12 GB。Mod 数量是游戏目录 `mods/` 下的 `.jar` 个数，禁用的 `.jar.disabled` 不算。

对照常见整合包的官方建议：

| 情况 | 算出 | 整合包建议 |
|---|---:|---|
| 原版 1.21 | 3 GB | 2–4 GB |
| Fabric 约 70 个优化 Mod | 4 GB | 3–4 GB |
| NeoForge 150 个 Mod | 6.5 GB | 6–8 GB |
| Forge 1.20.1 约 430 个 Mod（ATM9 级） | 12 GB | 10–12 GB |

**最多能给多少：**

```
上限 = 物理内存 − 系统预留 − Java 自身
系统预留 = 物理内存 × 20%，限制在 1.5–4 GB
Java 自身 = 768 MB + 每个 Mod 2 MB，最多 2 GB（类元数据、编译代码、线程栈、本地内存）
```

上限向下取整到 512 MB，至少 1 GB。32 位 Java 最多 1 GB。读不到物理内存时按 4 GB（以前的默认值）处理。

最大堆取「需要」和「上限」中较小的那个。上限不够时设置里会提示「电脑内存偏少」。

**初始堆（`-Xms`）：** 启动这一刻的空闲内存比「堆 + Java 自身 + 1 GB」多时，初始堆等于最大堆，运行中不用扩容；否则取一半，用多少占多少。看的是此刻空闲而不是总内存：初始堆在启动时就要占住，ZGC 还会立刻向系统要实打实的内存。实测本机总内存 16 GB、空闲约 2 GB 时，`-XX:+UseZGC -Xms12g -Xmx12g` 直接报 `Failed to commit memory` 退出；G1 在 Linux 上因为内存超额分配能启动，但 Windows 会把初始堆计入提交上限。读不到空闲内存时按总内存估算。

## 3. 回收器

**默认是 G1。** 参数取自官方启动器多年使用、覆盖 Java 8–21 的那一套，Region 按堆大小放大：

| 参数 | 值 | 作用 |
|---|---|---|
| `-XX:+UseG1GC` | | Java 8 默认是 Parallel GC，会长时间整堆暂停 |
| `-XX:G1NewSizePercent` | 20 | 新生代至少占 20%。Minecraft 分配速度很快，新生代太小会频繁回收 |
| `-XX:G1ReservePercent` | 20 | 多留空闲区，避免晋升失败 |
| `-XX:MaxGCPauseMillis` | 50 | 单次停顿目标约 3 帧 |
| `-XX:G1HeapRegionSize` | 8 / 16 / 32 MB | 堆 ≤4 GB 用 8，≤8 GB 用 16，更大用 32。超过半个 Region 的对象按巨型对象单独分配，Region 太小会让大数组频繁走慢路径 |
| `-XX:+ExplicitGCInvokesConcurrent` | | 游戏和 Mod 调 `System.gc()` 时做并发回收，不整堆暂停；直接内存仍能正常释放，所以不用 `DisableExplicitGC` |
| `-XX:+ParallelRefProcEnabled` | | 只给 Java 8–10。Java 11 起 G1 默认开启，Java 26 起这个参数被废弃 |
| `-XX:+UseCompactObjectHeaders` | | 只给 Java 25。JEP 519 在 25 转为正式功能，对象头从 12 字节减到 8 字节，堆占用更少 |
| `-XX:+PerfDisableSharedMem` | | 不把性能计数器写进 `/tmp` 的映射文件，避免磁盘忙时卡顿 |

`G1NewSizePercent` 是实验参数，需要 `-XX:+UnlockExperimentalVMOptions`，而且解锁参数必须在它前面，所以合并参数时解锁参数从不去重或挪位置。

**自动选 ZGC 的条件，必须全部满足：**

- Java 21 及以上：分代 ZGC 从 21 开始。21、22 需要 `-XX:+ZGenerational`，23 起是默认，24 起这个参数被废弃，JVM 会打警告。
- 不是 GraalVM：GraalVM 用 ZGC 时会关掉 Graal 编译器。
- 至少 12 个逻辑处理器、24 GB 物理内存。
- 算出的堆至少 8 GB，而且上限放得下多 1/4 的堆：ZGC 并发回收需要额外余量。

公开的客户端测试里，不分代 ZGC 在 8 核笔记本上明显掉帧，G1 在核少、内存少的机器上更稳。所以只有大整合包、强机器才自动换 ZGC。设置里可以手动选 ZGC；Java 17–20 只有不分代的 ZGC，会提示帧率可能更低；Java 8、11 和 32 位 Java 不支持，会退回 G1。

**没有采用的参数：**

| 参数 | 原因 |
|---|---|
| `AlwaysPreTouch` | 启动时把整个堆写一遍，大堆要多等几秒，还会立刻占满物理内存 |
| `DisableExplicitGC` | 有的库靠 `System.gc()` 释放直接内存，关掉后可能 `OutOfMemoryError: Direct buffer memory` |
| `UseLargePages` / `UseTransparentHugePages` | Windows 要管理员权限，失败时静默退回；Linux 需要配合 PreTouch |
| `UseStringDeduplication` | 公开测试里更慢 |
| `ThreadPriorityPolicy` 等线程优先级 | Linux 需要 root |
| 各种 JIT 节点限制、代码缓存调整 | 收益很小，而且版本间变动频繁，出错就启动不了 |

### 官方默认（26.1 起）

从 26.1 开始，Mojang 在版本文件的 `arguments.default-user-jvm` 里写了官方启动器的默认参数。26.1–26.3 都是：

```
-Xms2G -Xmx4G -XX:+UseCompactObjectHeaders -XX:+AlwaysPreTouch -XX:+UseStringDeduplication
-XX:+UseZGC                      macOS、Linux、Windows 10 1803 及以后
-XX:+UseG1GC 及官方 G1 参数        Windows 10 1803 以前
```

版本文件带了这一段时，「自动」按它走：回收器用官方选的，紧凑对象头、预先占用、字符串去重照用；最大堆仍按第 2 节计算（原版正好是官方的 4 GB，整合包按 Mod 数量放大）；初始堆沿用官方的 2 GB，因为 `AlwaysPreTouch` 会在启动时把初始堆写一遍。手动选 G1 时换成本节上面的 G1 参数，官方的通用参数保留。1.21.11 及以前的版本文件没有这一段，仍按上面的规则。

## 4. 用户参数

设置里的「额外 JVM 参数」按空白拆分，支持引号，排在所有参数最后。和自动参数冲突时：

- 写了 `-Xmx` 或 `-Xms`：堆大小整组交给用户，避免初始堆大于最大堆。
- 写了任何 `-XX:+Use…GC`：自动的回收器参数整组去掉。
- 写了同名参数（例如 `-XX:-PerfDisableSharedMem`）：去掉自动的那一个。

版本 JSON 自带的 JVM 参数同样优先。

## 5. 界面

设置里单独一张卡片：

- 内存：「自动」加 1 GB 步进器。自动时步进器显示算出的值，按加减就转成手动。
- 回收器：自动 / G1 / ZGC。
- 当前版本的结果，例如「G1 · 5 GB」，下面一行是依据：「按 16 GB 内存、16 线程、Java 21、64 个 Mod 计算」。需要注意的事用 `warn` 色列出。
- 「查看计算」展开每一步、完整参数和「额外 JVM 参数」输入框。

启动台底部和版本详情显示「内存 5 GB（自动）」。启动时日志记一行结果和依据。

`ui.json` 新增 `gc` 和 `jvm_args`；`memory` 为 `null` 表示自动。0.33 及以前没有 `gc` 字段，那时的 `4096` 是默认值而不是用户选的，迁移成自动；其他数值保留为手动。

命令行：`--memory` 不填就自动；新增 `--gc auto|g1|zgc` 和可重复的 `--jvm-arg`；`--dry-run` 打印计算过程。

## 6. 验证

- 单元测试覆盖上面各档机器、各 Java 版本、32 位、未知机器、用户参数覆盖和解锁参数顺序。
- 忽略测试 `generated_flags_start_every_installed_java` 用本机每个 Java 的三种回收器选择各跑一次 `java <参数> -version`。本机 OpenJDK 11、17、21、25 和 Oracle GraalVM 21 全部通过，没有警告；GraalVM 加 ZGC 只有预期的 JVMCI 提示。
- 实机对比（Linux、16 线程、16 GB、Xvfb + llvmpipe 软件渲染，NeoForge 1.21.1、GraalVM 21，同一个新世界，进世界后疾跑 100 秒持续生成新区块，各跑一次）：

  | | 旧参数 `-Xms1024m -Xmx4096m` | 自动 G1 · 3.5 GB |
  |---|---:|---:|
  | 进世界后回收次数 | 15 | 8 |
  | 进世界后停顿合计 | 100 ms | 68 ms |
  | 进世界后最长停顿 | 15.9 ms | 13.4 ms |
  | 超过一帧（16.7 ms）的停顿 | 0 | 0 |
  | 加载阶段停顿合计 | 147 ms | 169 ms |

  旧参数的堆整局停在初始的 1 GB，新生代被压得很小，回收更频繁；新参数初始堆等于最大堆，回收次数减半，总停顿少约三分之一。加载阶段新参数略多，因为新生代更大、单次要复制的更多，这一段不影响游戏中的帧。样本只有一次，软件渲染下帧率本身很低，只能说明方向，不能当作精确的提升幅度。
- Java 8 本机没有安装，参数与官方启动器在 Java 8 上用了多年的那组相同，未实测。

## 7. Java 选择

按版本要求的大版本选「够用里最低的」：要 Java 8 而本机只有 17 和 25 时选 17，不选最新的 25（老 Forge 在太新的 Java 上起不来）。同一个大版本里优先普通的 64 位 OpenJDK，其次才是 GraalVM。

## 8. 依据

- Mojang 版本清单 `piston-meta.mojang.com` 中 26.1–26.3 的 `default-user-jvm`
- [JEP 474：ZGC 默认分代（JDK 23）](https://openjdk.org/jeps/474)，[JEP 490：移除不分代 ZGC（JDK 24）](https://openjdk.org/jeps/490)
- [JEP 519：紧凑对象头转正（JDK 25）](https://openjdk.org/jeps/519)
- [JDK-8359925：废弃 ParallelRefProcEnabled](https://bugs.openjdk.org/browse/JDK-8359925)
- [brucethemoose/Minecraft-Performance-Flags-Benchmarks](https://github.com/brucethemoose/Minecraft-Performance-Flags-Benchmarks)：客户端 G1 与 ZGC 的对比、哪些参数有害
- [PaperMC：Aikar's flags](https://docs.papermc.io/paper/aikars-flags)：G1 各参数的含义（服务端取值，客户端未照搬）
