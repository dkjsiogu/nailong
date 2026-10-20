# 给老板看的设计简报

这不是技术文档。它用图和日常语言说明：我们要做什么、为什么这样选、风险在哪、什么时候能看到东西。

## 怎么打开

- **PDF**：[`nailong-brief.pdf`](nailong-brief.pdf)（9 页，16:9，适合投屏或转发）
- **一图总览**：[`nailong-brief-contact.png`](nailong-brief-contact.png)（9 页缩略图）
- **网页源文件**：[`nailong-brief.html`](nailong-brief.html)（可改字后重新导出）

## 页面

1. 封面：一句话 + 三个数字
2. 用户只需要三步
3. 轻，是算出来的（体积/内存/打开速度）
4. 里面怎么分工（前台/调度/仓库/门卫）
5. 点下“开始”之后发生什么（5 步）
6. 三个关键取舍
7. 风险：先把丑话说在前面
8. 约 18 周，分六段走
9. 需要你拍板的三件事

## 重新生成

```bash
cd docs/brief
google-chrome --headless=new --no-sandbox --disable-gpu \
  --no-pdf-header-footer --virtual-time-budget=3000 \
  --print-to-pdf=nailong-brief.pdf "file://$PWD/nailong-brief.html"
pdftoppm -png -r 96 nailong-brief.pdf slide
```

## 和技术文档的关系

- 给老板/评审：本目录的 PDF。
- 给技术同事：`docs/design/` 下的 00–13。
- 本简报只讲“为什么”和“看到什么”，不展开实现细节。
