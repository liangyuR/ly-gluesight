# 教程维护

直接打开 [图文教程](index.html)。它可以离线阅读，截图可点击放大，左侧目录可跳转。打印按钮使用同一内容生成 A4 横向手册。

内容主文件为 `guide.json`，图片放在 `images/`。`guide.css` 同时控制网页与打印版式，`guide.js` 负责图片放大。`index.html` 由这些源文件生成；更新时修改源文件，避免只改生成结果。

在仓库根目录运行：

```powershell
pnpm guide:build
pnpm guide:pdf
```

仓库保留可直接阅读的 [PDF 教程](../../output/pdf/GlueSight-图文使用教程.pdf)。更新源稿后，重新导出到 `output/pdf/GlueSight-图文使用教程.pdf`，将源稿、HTML、截图与 PDF 一起提交。导出脚本使用本机 Edge 或 Chrome 的独立临时配置，不打开可见浏览器窗口。也可用 `pwsh -File scripts/user-guide/Export-Guide.ps1 -Browser "浏览器完整路径" -Output "目标 PDF 路径"`。

需要浏览器预览时运行 `pnpm guide:serve`，打开 <http://127.0.0.1:8772/>。关闭该终端的服务即可结束预览；离线阅读直接打开 `index.html`，并保留同目录的样式和图片。

更新步骤：先验证软件行为，再在隔离的演示实例中通过实际界面操作截取对应区域；替换同名 PNG；修改步骤、图注与版次；重新生成网页与 PDF；检查所有分页、图片及文字。不要用设计稿或浏览器流程预览替代实际桌面截图。

教程以 ROBOT-DEMO 为主。随动原图试测图来自界面名称更新前的独立离线回放实例，图注已注明。示例参数和合成图像不能作为现场工艺或精度验收数据。
