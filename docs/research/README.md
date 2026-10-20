# 研究材料（内部）

本目录是 nailong 立项阶段的**只读源码研究快照**，用于支撑 [设计文档](../design/) 中的判断。它们不是产品文档，可能包含：

- 参考项目的文件路径、类名、行号；
- 对行为的分析、少量伪代码/数据结构；
- 临时结论与待验证项。

## 来源

| 文件 | 对象 | 许可证 | 说明 |
|---|---|---|---|
| `prism.md` | [Prism Launcher](https://github.com/PrismLauncher/PrismLauncher) | GPL-3.0 | C++/Qt；meta 驱动的组件模型 |
| `hmcl.md` | [HMCL](https://github.com/HMCL-dev/HMCL) | GPL-3.0 | Java/JavaFX；分层、patch、Forge processors |
| `pcl-rust.md` | [PCL](https://github.com/Meloong-Git/PCL) + Rust 生态 | PCL 分发有限许可；其余见文内 | VB.NET/WPF；GUI/技术选型实测 |

## 使用约束

1. 项目已定 **GPL-3.0-or-later**：可在履行 GPL 义务的前提下参考/移植 Prism/HMCL 的 GPL 代码（保留版权声明、提供源码）。
2. PCL 的许可证不是 OSI 开源许可；**不得**复制其代码、资源或文案。
3. 报告中的行号/端点会随上游版本变化；设计文档以“机制”为准，不绑定具体行号。

## 与设计文档的关系

- 横向对比与结论：[`01-prior-art.md`](../design/01-prior-art.md)
- GUI 实测数据：[`gui-2026-10.md`](../bench/gui-2026-10.md)
- 2026 版本 JSON 实测：[`appendix-a-version-json-2026.md`](../design/appendix-a-version-json-2026.md)
