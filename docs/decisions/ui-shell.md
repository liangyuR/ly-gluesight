# ui-shell 工作线决策记录

范围：qing-steel、应用外壳（侧栏、导航、页面切换、全局动效、对话框、提示、加载与空状态）、生产运行页（在线检测）、历史记录列表页。目标屏幕 1920×1080 工控机。

对应 PR：

- qing-steel：发布 0.3.0（对话框、提示条、空状态、统计读数、进度与大屏判定）
- GlueSight A `feat/ui-shell-motion`：外壳动效只用 transform/opacity，对话框与提示改用 qing-steel 组件
- GlueSight B `feat/production-run-screen`：生产运行页一屏重排
- GlueSight C `feat/history-list-polish`：历史记录列表

## 1. 侧栏收起 / 展开只用 transform 与 opacity

- 问题：原实现过渡 `width`，动画期间每一帧都重排整个主区。隔离实例生产页实测一次收起触发 82 次布局、39 ms 布局耗时；内容在 240 ms 内连续换行，本身也是一种抖动。
- 可选项：(a) 保留 width 过渡；(b) 动画 `clip-path`；(c) 布局宽度在文字淡出后一次切换，侧栏底板 `translateX` 平移，主区用 FLIP 做反向 `translateX`；(d) 取消动画直接切换。
- 采用：(c)。只动 transform 与 opacity，布局只发生一次；仍保持 qing-steel 的时序（文字淡出 → 宽度 → 文字淡入）和时长令牌，`prefers-reduced-motion` 下直接切换。实测收起 82 → 5 次布局，布局耗时 39 → 3.2 ms。代价：切换那一帧主区一次性重排（约 1 帧 30 ms，发生在文字已淡出时）；展开时主区右缘在 240 ms 内露出最多 148 px 的页面底色（与主区背景同为深色，肉眼几乎不可见）；当前项胶囊在切换期间淡出、到位后淡入，不再伸缩。
- 回退：还原 `src/layout/Sidebar.tsx`、`src/layout/AppLayout.tsx` 与 `src/styles.css` 侧栏段。

## 2. qing-steel `.qs-nav` 暂不改造

- 问题：库内 `.qs-nav` 的 rail 切换同样过渡 `width` / `padding`。
- 可选项：照搬第 1 条做法（需要导航不再是滚动容器、由脚本对相邻主区做 FLIP，属结构性破坏变更）；本版不动。
- 采用：本版不动，在 qing-steel PR 中写明。GlueSight 侧栏不使用 `.qs-nav`，不受影响。
- 回退：无。

## 3. 分段选中指示器改为 translate + scale

- 问题：`SelectionTrack` 用 WAAPI 动画 `width` / `height`，每帧布局。
- 采用：指示器在动画开始时一次落到目标尺寸，动画只插值 `translate(...) scale(...)`（`transform-origin: 0 0`）。`.segmented` 内指示器改为直角，两端由容器圆角裁切（与 `qs-seg` 相同）；`.wp-steps` 的圆角指示器在 240 ms 内有轻微圆角形变，可接受。
- 回退：还原 `src/components/SelectionTrack.tsx` 与 `styles.css` 中 `selection-track-indicator` 两行。

## 4. 局部超差（OK_WITH_EXCURSION）统一用警告色

- 问题：生产页判定卡把局部超差显示为 OK 绿，历史列表、拍照点、检测段都用警告黄。
- 可选项：统一绿；统一黄；绿卡 + 黄标签。
- 采用：判定卡用 `warn` 容器色，大字仍是 `OK`（它是合格件，PLC 收到合格码），副标题“局部超差”。与拍照点、检测段、历史列表一致。
- 回退：`InspectPage.tsx` 的 `verdictTone` 把 `OK_WITH_EXCURSION` 改回 `ok`。

## 5. 未测成与设备故障用故障紫，与产品 NG 分开

- 问题：设备故障时设备状态条用 NG 红；帧状态“等待测量 / 定位失败 / 未到达”统一用警告黄。
- 采用：遵循 qing-steel 原则——产品 NG 红、系统 / 设备故障紫。整件“未测成”、设备状态条故障（新增 `device-fault`）、帧状态定位失败 / 测量出错 / 未到达用 fault；等待与测量中改为中性色。
- 回退：`InspectionReadiness.tsx` 的 `status` 故障分支改回 `blocked`；`RuntimeFrame.tsx` 恢复 Badge。
- 2026-10-21 补记（特征定位 G1）：定位失败不再是帧状态。帧照常测完，没有位置基准的视图记在 `referenceFailed`，在帧缩略图、选中帧、判定卡和历史详情上挂 fault 色“定位失败”徽标，整件判 NG（NG 优先于基准无效）时也照样显示；有胶无基准的站用 fault 色实线，超出检测范围的站用灰色点线，表示不参与判定。

## 6. 判定字 80 px（`--text-verdict`）与右栏 420 px

- 问题：原判定字 24 px，2 m 外无法分辨 OK / NG。
- 依据：24 英寸 1080p 屏像素间距约 0.28 mm，80 px 等宽大写字高约 16 mm，按常用 1:200 视距比约 3 m 可读；右栏 420 px 能放下 `ERR` 加两行说明。1600 px 以下右栏 360 px，1280 px 以下 300 px 且判定字 56 px；高度不足 960 px 时判定卡压到 196 px。
- 回退：去掉判定卡的 `xl` 类或改 qing-steel 令牌。

## 7. 生产页右栏顺序：判定 → 今日统计 → 本件进度 → 检测段

- 理由：远处首先看判定，其次看当班产量与 NG 数；检测段最多（现场 36 段），放最后并在栏内滚动。

## 8. 设备状态压成一行

- 问题：本件进行中频繁出现“等待 · 审计与录制”，原来每件都插入一行清单，整页下移再回弹。隔离实例实测每件 CLS 0.069（正常件）/ 0.083（NG 件）。
- 可选项：保留清单但预留一行高度；等待项内联到摘要行；改成浮层。
- 采用：摘要、未就绪项、诊断入口同一行，等待项内联（过长省略，完整内容在诊断抽屉）；阻塞或故障时允许换行完整展示（此时不在生产，变高可接受）。实测每件 CLS 降到 0.001–0.004（剩余为文字宽度变化引起的水平微移）。
- 回退：删除 `src/pages/inspect.css` 中 `.prod .inspection-readiness` 相关规则。

## 9. 判定卡固定高度

- 问题：原因文本长短不同，判定卡高度跟着变，下面的进度与检测段上下跳。
- 采用：固定 232 px（矮屏 196 px），原因最多三行，超出省略，完整原因在 `title`、诊断抽屉与历史详情。
- 回退：去掉 `.prod-side` 第一行固定高度。

## 10. 同屏只保留一个循环动画

- 采用：运行中只有判定卡的呼吸点循环；阶段徽章的点用 primary 实色、不呼吸。结果出现时新内容自下滑入，变 NG 时整卡 1.02 强调一次，均为一次性动画。

## 11. 不调整全局 CSS 的引入顺序

- 问题：`main.tsx` 先 `import App` 再引入 qing-steel 与 `styles.css`，页面样式先于全局与组件样式注入，同优先级时全局样式会覆盖页面样式。
- 可选项：调整 import 顺序；页面覆盖时提高选择器优先级。
- 采用：后者。调整顺序会改变其他页面（属于其他工作线）里大量同优先级规则的胜负，风险大。
- 回退：无。

## 12. 生产页样式从共享 cycle.css 拆出

- 采用：生产页专用规则（`.fly*`、`.inspect-operator*`、`.stats5`、`.seg-list`、`.bar` 等）删除，新规则放在 `src/pages/inspect.css`；历史详情仍在用的 `.kv2`、`.panel-head`、`.legend`、拍照点格与清单基础样式保留。`InspectionReadiness`、`ShotStrip`、`RuntimeFrame`、`OfflineControls` 只有生产页使用，局部调整。

## 13. 三个 GlueSight PR 都带同一份 qing-steel 固定

- 问题：B、C 用到 0.3.0 的新组件；要求各 PR 可独立合并。
- 采用：A、B、C 对 `package.json`、`pnpm-lock.yaml`、`src/main.tsx` 做完全相同的改动（固定到 qing-steel PR 提交 `87df313` 并引入 `components.css`），任意顺序合并都不冲突（已在本地按 A → B → C 合并验证）。qing-steel PR 合并并打 `v0.3.0` 标签后，把依赖改为 `#v0.3.0`。
- 回退：依赖改回 `#v0.2.0` 并去掉 `components.css` 引入（B、C 的新组件会失去样式）。

## 14. 无真机时的走查与帧率测量方法

- 采用：(1) 隔离 Tauri 实例（`Start-UiTestInstance.ps1 -Profile flow -DebugPort 9343`）+ 进程内虚拟 S7 产线（合成图），CDP 驱动 `offline_run` 跑正常 / 断胶 / 丢帧 / 定位失败，WebView2 视口固定 1920×1080；(2) vite + 从隔离实例采集的真实快照、测量值和原图回放的模拟后端，用无头 Edge 1920×1080 / 1440×900 截图迭代。
- 指标：requestAnimationFrame 帧间隔（最长帧、超过 20 ms 的帧数）、CDP `Performance.getMetrics` 的布局次数与耗时、`layout-shift` 累计 CLS。WebView2 隐藏窗口与无头浏览器的 rAF 不锁 60 Hz，帧率数字只作相对比较。

## 15. 曲线段名按估算字宽截断

- 问题：现场 36 段时段名互相重叠（原规则只要段宽 > 36 px 就画完整段名）。
- 采用：按中文 10.5 px、其他 6.2 px 估算宽度，放不下时截断加省略号并保留 `<title>` 完整名，两个字都放不下就不画。不做 DOM 测量，避免每次渲染强制布局。
- 回退：还原 `UnrolledCurve.tsx` 中 `fitLabel`。

## 16. 历史列表 NG 整行底色

- 采用：按 qing-steel 原则“产线上要找的是坏件”，NG 记录整行 `ng-container` 加红色左标；未测成紫色左标、局部超差黄色左标，OK 不标。离线标签改用 tertiary（非常规模式），历史详情页同名标签随之变色。
- 回退：删除 `history.css` 中 `.hist-table tbody tr.r-*` 规则。

## 17. 加载提示不插入新段落

- 问题：历史列表加载时在筛选区插入“正在加载历史记录…”段落，表格整体下移。
- 采用：状态文字放进统计行末尾（`role="status"` 保留），表格加载时半透明；不使用旋转图标（qing-steel 不做装饰循环）。

## 附：测量数据

隔离 Tauri 实例（WebView2，视口 1920×1080，进程内虚拟 S7 产线 + 合成图，MTR-HSG-A 6 个拍照点）。改前为 main `1f8b2f7`，改后为 A、B、C 本地合并版，改后共测三轮；测量期间其他工作线占用 CPU 约 50–63%，最长帧随负载波动。

| 场景 | 改前：布局次数 / 布局耗时 / CLS / 最长帧 | 改后：布局次数 / 布局耗时 / CLS / 最长帧 |
| --- | --- | --- |
| 侧栏收起（生产页有数据） | 82 / 39.0 ms / 0 / 12.1 ms | 5 / 1.8–5.2 ms / 0 / 11.5–12.1 ms |
| 侧栏展开（生产页有数据） | 80 / 43.0 ms / 0 / 23.5 ms（1 帧 > 20 ms） | 6 / 2.1–2.6 ms / 0 / 6.8–11.6 ms |
| 切页 历史 → 在线检测 | 46 / 11.5 ms / 0 / 17.4 ms | 8–10 / 5.6–11.3 ms / 0 / 17.5–36.6 ms |
| 生产一件（正常） | 337 / 53.5 ms / 0.069 / 16.6 ms | 41–48 / 22.4–34.3 ms / 0.0001–0.0014 / 18.1–45.3 ms |
| 生产一件（断胶 NG） | 333 / 57.4 ms / 0.083 / 16.5 ms | 44–45 / 22.9–38.5 ms / 0.0001–0.0038 / 17.9–34.5 ms |

样式重算耗时略有上升（判定滑入、进度条 scaleX 等一次性动画计入样式重算），布局次数与 CLS 明显下降。vite + 模拟后端（无头 Edge，回放隔离实例采集的真实快照）中，空闲 → 检测中 → 结果的切换 CLS 由每次 0.0165 降为 0。