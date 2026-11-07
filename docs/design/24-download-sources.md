# 24 · 下载源和镜像

> 0.39 起，Cairn 所有的下载和元数据请求都有后备：官方连不上就换镜像，镜像不行再回官方。用的两个镜像都是社区免费提供的公共服务，这一页写清楚怎么用它们、答应过它们什么。

## 1. 用了哪两个镜像

| 镜像 | 管什么 | 维护者 | 需要申请吗 |
| --- | --- | --- | --- |
| [BMCLAPI](https://bmclapidoc.bangbang93.com/) | Mojang 的版本清单、游戏文件、库、资源，Fabric / Forge / NeoForge 的 Maven | bangbang93 | 不需要；要求在下载界面写明来源，不做二次封装 |
| [MCIM](https://github.com/mcmod-info-mirror) | Modrinth 和 CurseForge 的接口 | mcmod-info-mirror | 需要登记 UA，Cairn 已在 [mcim-rust-api#12](https://github.com/mcmod-info-mirror/mcim-rust-api/issues/12) 登记 |

## 2. 先连谁

设置里有两种下载源：

- **官方优先**（默认）：先连官方，连不上自动换 BMCLAPI。
- **镜像优先**：游戏文件和加载器先走 BMCLAPI，国内通常更快；BMCLAPI 不行再回官方。

不管选哪种，**Modrinth 和 CurseForge 总是先连官方**，官方连不上才问 MCIM。这是登记时答应 MCIM 的。

MCIM 的文件地址目前只是 302 跳回官方 CDN，所以 Mod 文件不走 MCIM，只有接口（搜索、项目、版本列表）走。

规则写在 `core::http::fallback_urls`，有单元测试。

## 3. 换源的细节

- **官方说「没有」就作数**：官方回 400、404、410、422 时不再问镜像。镜像说没有时，先问一下官方（镜像有时比官方晚），官方也连不上才把镜像的答案交出去。401、403 不算「没有」：常是地区限制或密钥问题，镜像可能帮得上。
- **连不上记五分钟**：一个官方主机连不上、镜像成功之后，五分钟内同一主机直接先走镜像，不用每个请求都等它超时；五分钟后再试官方，连上了就换回来。批量下载几千个资源文件时，这一条决定了快慢。
- **密钥只发给原本的主机**：CurseForge 的 `x-api-key`、`Authorization`、`Cookie` 只发给请求原本的主机，换到镜像就去掉。
- **校验不变**：换了源也按官方清单里的 SHA-1 和大小核对，对不上就丢掉重下。校验失败不算「连不上」，不会因此换源。
- **UA 一致**：所有请求都带 `cairn/<版本> (+https://github.com/dkjsiogu/cairn)`，和登记的一致。
- **同步时间**：MCIM 的每个对象带 `sync_at`。Cairn 记下最近一次回答里最旧的那个，内容页的提示会写「几小时前同步」。

## 4. 人能看到什么

- **下载页**：「刚下载」旁边写着「来自 Mojang 官方 / BMCLAPI 镜像 / Modrinth / …」，悬停看主机名。这也是 BMCLAPI 要求的来源说明。
- **内容页**：Modrinth 或 CurseForge 的官方连不上、正在用 MCIM 时，顶部有一行淡淡的提示，比如「Modrinth 官方暂时连不上，正在用 MCIM 镜像 · 3 小时前同步」；悬停说明数据可能晚一些、几分钟后会再试官方。
- **两边都连不上**：内容页写「官方和镜像都没连上，请检查网络或代理后重试」，不是一串地址。
- **设置**：「下载源」两个选项，下面一行致谢，BMCLAPI 和 MCIM 都能点开看说明（BMCLAPI 的页面里有赞助方式）。

## 5. 答应过的事

对 BMCLAPI：下载界面写明来源；不做二次封装，不把它包成自己的服务。

对 MCIM：只在官方连不上时作为后备，官方始终优先；UA 和登记的一致；文件照常校验；看 `sync_at`；不提供二次封装。

以后要改这些规则，先回来读这一节。

## 6. 实测（0.39）

- 用本地代理拦掉 `api.modrinth.com`：搜索 0.8 秒拿到结果（来自 MCIM），之后同一主机直接走 MCIM；不存在的项目 1.1 秒报 404，不再白白重试。
- 不拦时：完全不碰 MCIM。
- 拦掉 Mojang 的全部五个主机（清单、文件、库、资源）：`cairn-cli install vanilla 1.12.2` 53 秒装完，全部从 BMCLAPI 和它的 OpenBMCLAPI 节点下载。
- 唯一缺的是 1.12 的日志配置 `client-1.12.xml`（BMCLAPI 回 404）。它本来就是可选的，启动时没有它也会带 `-Dlog4j2.formatMsgNoLookups=true`。
