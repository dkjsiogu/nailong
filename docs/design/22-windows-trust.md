# 22 · Windows 下载提示

> 0.36 在 Windows 上实测：从 GitHub 下载 exe 时，浏览器提示「通常不会下载 nailong (1).exe。请在打开前确保信任」，下载 HMCL 时不会。这一页说清楚为什么、0.37 做了什么、以后怎样才能不弹。

## 1. 这是什么提示

这是 Microsoft Defender SmartScreen 的**应用信誉**检查（Edge 和 Windows 都用它）。它不是说文件有毒，而是说「这个文件下载过的人太少，我还不认识它」。

信誉记在两个地方：

- **文件本身**（按哈希）：每发一个新版本，exe 的哈希就变了，信誉从零开始；下载和运行的人多了，提示才会消失。
- **签名证书**：用受信任 CA 签过名的文件，信誉还会记在证书上，新版本可以沿用。

2024 年起微软改了规则，EV 证书也**不再**一签就免提示，和普通 OV 证书一样要靠下载量慢慢攒。（[微软文档](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation)、[ToDesktop 的说明](https://www.todesktop.com/blog/posts/windows-apps-psa-ev-certs-do-not-grant-immediate-reputation-anymore)）

## 2. 为什么 HMCL 不弹

实际检查了 HMCL 3.16.4 的 `HMCL-3.16.4.exe`：PE 头里的证书表大小是 0，**它也没有签名**。

区别在下载量：这个 exe 在 GitHub 上下载了一万五千多次，jar 一万八千多次；每个新版本发布几小时内就有几千人下载运行，SmartScreen 很快就「认识」了这个哈希。我们的 exe 每个版本只有很少的人下，所以总在「还不认识」的阶段。文件名带「(1)」只是因为同一个文件下载了两次。

## 3. 0.37 做了什么（不花钱的部分）

- exe 里写进了图标和完整的版本信息（产品名 Cairn、文件说明、版本号、原始文件名、作者），资源管理器的「属性 › 详细信息」能看到。CI 会检查，没写进去就不发版。这让文件看起来像一个正经程序，但**不会**让提示立刻消失——那只取决于信誉。
- 文件名固定为 `cairn.exe`，不再每版换名；SHA256SUMS 照常发布，可以核对。
- 发布流程里预留了代码签名：配好 SignPath 的变量和令牌后，Release 会自动把 exe 送去签名再发布；没配时这一步整个跳过，不影响发版（`.github/workflows/release.yml` 的 `sign` 任务）。

## 4. 怎样才能不弹

按推荐顺序：

1. **SignPath Foundation（免费，推荐）**：给开源项目免费的 OV 代码签名证书，在云端签名，私钥不落到 CI 上。要求公开仓库、OSI 认可的许可证（GPL-3.0 可以），一般 1–2 周审批；签名者显示为 SignPath Foundation，README 里要注明。签名后信誉记在证书上，仍要攒一段下载量，但之后的新版本不会每次从零开始。申请通过后在仓库设置里填：
   - Variables：`SIGNPATH_ORGANIZATION_ID`、`SIGNPATH_PROJECT_SLUG`、`SIGNPATH_POLICY_SLUG`
   - Secrets：`SIGNPATH_API_TOKEN`
   - 并在仓库里安装 SignPath 的 GitHub App。
2. **Certum 开源代码签名证书**：个人可买，每年几百元人民币，全球可申请；但它用云端的 SimplySign 签名，接进 CI 比 SignPath 麻烦。
3. **Azure Artifact Signing（原 Trusted Signing）**：每月 9.99 美元，但个人开发者目前只开放给美国和加拿大，国内个人用不了。
4. **winget**：把 Cairn 提交到 Windows 包管理器（微软的 winget-pkgs 仓库），用 `winget install` 安装时不经过浏览器下载检查。免费，需要给微软的仓库提一个 PR，每个版本更新一次清单。
5. **微软应用商店**：商店分发不会有这个提示，但要打包成 MSIX、走审核，和现在「一个 exe 就能用」的定位不太一样。

在这之前，用户能做的：浏览器里选「保留 → 仍然保留」；第一次运行时 Windows 提示「未知发布者」就点「更多信息 → 仍要运行」；也可以把文件提交给微软的[误报申诉](https://www.microsoft.com/en-us/wdsi/filesubmission)，能加快这个版本攒信誉。
