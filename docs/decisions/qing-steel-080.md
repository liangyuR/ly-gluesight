# qing-steel React 组件迁移决策记录（qing-steel-080）

外壳与各页面的原生控件和本地样式换成 qing-steel React 组件（迁移时为 0.8.0，现依赖 0.9.0），样式入口为 `qing-steel/react.css`；`main.tsx` 用 `ThemeProvider`（深色）包住应用，Radix 浮层挂在主题容器内，`App.tsx` 接入 `SnackbarProvider`。本文记录迁移约定、保留本地实现的组件与 qing-steel 缺口。不改业务规则、不改后端。

## 一、迁移约定

### QS-1 覆写 qing-steel 组件样式必须提高选择器权重

- 问题：`main.tsx` 先 `import App`（带出各 feature 的 CSS），再 `import "qing-steel/react.css"` 与 `./styles.css`。同权重的规则，后加载的组件样式和 `styles.css` 获胜，feature CSS 里的写法会被盖掉。
- 采用：feature CSS 覆写组件时加双类名或父级限定，例如 `.qs-modal-content.rj-dialog`、`.qs-surface.hist-results`、`.wp-panel.wp-progress-board`（进度总览大标题与内边距）、`.wp-panel.wp-teach-panel > .qs-card-heading`、`.prod .qs-btn.runtime-view-attention`（按钮文字色取自 `--btn-fg`，`.qs-btn.tonal` 的权重是两个类）。
- 不采用：调整 `main.tsx` 的 import 顺序。样式入口的顺序影响全部页面，这次迁移不动；要改需整体走一遍样式回归。
- 回退：无需回退，只是写法约定。

### QS-2 端到端与单元测试怎样定位新组件

- Select 是 Radix 自绘：可访问的 `combobox` 是触发按钮，没有 value，Playwright 的 `selectOption` / `inputValue` 与 `userEvent.selectOptions` 对它无效。qing-steel 还保留一个 `aria-hidden` 的 1 px 原生 `select`，`aria-label` 与触发按钮相同。
  - 选项文字等于要选的值时：点开 `getByRole("combobox", { name })`，再点 `getByRole("option", { name, exact: true })`（例如历史配方、诊断包记录范围、离线工况）。
  - 选项文字比值长（带代码、时间等）或要读当前值时：定位 `select[aria-label="…"]`，用 `selectOption` / `inputValue`（例如离线配方、历史帧选择、已保存对照结果）。Select 用 `label` 而不是 `aria-label` 命名时，原生 select 没有名称：给了 `id` 就用 `#<id>-native`，否则只能走点开再选。
  - Radix 的选项弹层挂在主题容器里，不在对话框内，选项要从 `page` 上定位，不要从 `dialog` 上定位。
- 表格行不再有 `role="row"` 的可访问名称（`Table` 不接受行级属性）：历史列表用首列按钮 `查看 SN … 的记录` 定位和点击；在行内再查文字时用 `getByRole("row").filter({ has: 按钮 })`。
- 工作台的面板用 `.wp-panel`（`Panel` 现在是 `Card`，不再是 `section`），再用标题过滤。
- 单元测试渲染用到 `useSnackbar()` 的组件时必须包 `SnackbarProvider`（没有 Provider 时会抛错）；Radix 浮层需要 `matchMedia` 桩，已在测试设置里补。

### QS-3 NumberField 与“越界仍可输入并提示”的表单

- 问题：qing-steel `NumberField` 给了 `min` / `max` 会在失焦时自动夹紧，值类型是 `number | ''`，`step` 只接受 number（原先的 `step="any"` 表达不了）。原先的行为是超范围时保持输入、显示校验错误并阻止保存。
- 采用：共享封装 `NumberField`（`features/workspace/components.tsx`，整圈采集、示教设置在用）保留 `FormField + TextField type="number"`，沿用原行为；其余页面（相机、PLC、系统设置、批量重判、诊断包导出、配方库、配方编辑、示教页内联参数）直接用 qing-steel `NumberField`，接受失焦时夹回 `min` / `max` 范围。
- 回退：把共享封装换成 qing-steel `NumberField`，同时要接受整圈采集与示教设置里“越界被夹回范围内”的行为变化。

## 二、保留本地实现的组件

| 组件 / 位置 | 保留原因 |
| --- | --- |
| 侧栏 `Sidebar`（只把收起按钮换成 `IconButton`） | `NavigationDrawer` 不满足：只能用 `a[href]` 或 `button`，不能用路由 `Link`；收起态要显示小字标签；分组没有 `role="group"`，收起后没有分隔线；底部放不了带选中态的导航项（系统设置）；收起按钮名称固定 |
| `src/components/SelectionTrack.tsx`（工作台步骤条 `Steps`） | 步骤条是带状态的 `NavLink`，不适合 `SegmentedControl`；qing-steel 内部的 `SelectionTrack` 未导出。历史、检测、整圈采集的分段切换已改用 `SegmentedControl`，这里是仅存的使用方 |
| `WorkspaceBar` 的“当前配方”原生 `select` | Radix `Select` 的 combobox 是按钮、没有 value，`toHaveValue` 与 `selectOption` 用不了 |
| 外观是按钮的 `Link`（“配方库”“打开配方库”用 `.btn`，离线页与总览页的跳转按钮用 `qs-btn`，步骤胶囊链接、相机 `cg-link` 文本链接） | `Button` 不能渲染为链接（没有 `asChild` / `href`）；单元测试按 `link` 角色和 `href` 断言 |
| `KV` / `dl.kv` / `kv2` 键值网格 | 没有对应组件 |
| 判定卡（`InspectPage` 内的 `VerdictCard`，`.fly-verdict` / `.qs-verdict` 标记） | 见缺口“VerdictCard”：局部超差大字、SN 与 PLC 码头部等与 ui-shell §4 冲突 |
| 今日统计（`.qs-stats`） | `MeasurementReading` 面向测量值，用于计数会重复标签、0 件 NG 也标红 |
| `GrayViewer`、`StepChart` | 业务可视化，没有对应组件 |
| 相机总览实时 canvas 预览、新鲜度标签、计数格 | `CameraThumbnail` 只接受 `src` 或只读 `preview`；`MeasurementReading` 不覆盖空格千分位和文字值 |
| 上下文帮助 `ContextHelp` | `Tooltip` 不支持点击后固定展开，`Popover` 不能悬停 / 聚焦打开且角色是 dialog |
| 握手信号灯 `SignalLamps` 与顶栏阶段徽标（`.lamp` / `.qs-badge`） | `StatusBadge` 没有中性色调，无法表达灯未亮或空闲 |
| 候选调试横幅（`.qs-banner` tertiary） | `Alert` 没有 tertiary 色调，也不接收 `className` |
| `OfflineControls` 的行内状态与错误文字、开工清单的 `details` 折叠、诊断 kv 与事件日志布局 | 行内文字不是 `Alert`；`Accordion` 卸载折叠内容 |
| 原生 `<details>` 折叠区（整圈采集步骤、相机曝光增益、标定、检测参数与判定限值卡、已通过检查、日志结构化字段） | `Accordion` 折叠时卸载内容，没有 `forceMount`；放在 `fieldset disabled` 内时触发按钮被禁用，锁定状态下“高级参数（调试）”无法展开 |
| `RecipeEditor` 的判定限值表 | 需要 `colSpan`、行头和可编辑单元格，`Table` 都没有；只把输入换成 `NumberField`、开关换成 `Switch` |
| 整圈采集结果的键值标签（`.wp-capture-chip`） | `Chip` 只放一段文字 |
| 原生文件输入（总览背景导入、样本组逐点图片） | qing-steel 没有文件输入组件 |
| 相机详情“切换相机”分组 `cg-switch` | 选项要带状态圆点和“名称 编号”的可访问名称，`SegmentedControl` 的 label 只能是字符串 |
| 历史统计行 `hist-counts`、`reason-box`、`ng-item`、`history-segment` 业务色块（用 `data-tone`）、行级业务样式（NG 行底色、告警 / 故障色条、错误日志行底色，用 `tr:has([data-tone])` 与 `tr:has([data-level])`） | 件数与良率不是测量读数；`Table` 不支持行级属性 |
| 共享 `Panel`（`features/workspace/components`） | 端到端脚本依赖 `.wp-panel` 加标题定位；内部已是 `Card`，标题槽位的字号靠 `styles.css` 里的 `.wp-panel > .qs-card-heading` 规则维持 |
| 共享工具类 `wp-actions`、`spacer`、`mono`、`kv2`、`sn`、`muted`、`hint` | 各页面仍在使用 |
| `plc/components/Modal.tsx` | 封装 `Dialog` / `Drawer`：用 ref 设置宽度、对话框打开后把焦点移到第一个输入框，补 qing-steel 缺的宽度与初始焦点参数 |
| `window.confirm`（移除相机） | 属于行为，未替换 |

## 三、qing-steel 缺口清单

供以后向 qing-steel 提需求，已按组件去重；括号里是首先发现该问题的工作线。

### 外壳与反馈

- **SnackbarProvider / useSnackbar**
  - 没有位置或容器参数，宿主只能用 CSS（`:has` + 侧栏宽度）把 `.qs-snackbars` 放到主区顶部居中（外壳）。
  - 没有 Provider 时 `useSnackbar` 抛错（原本地实现静默忽略），单元测试必须挂 Provider（外壳）。
  - 一次只显示一条，没有堆叠选项（r2-ui）。
- **NavigationDrawer**：只能用 `a[href]` 或 `button`，不能用路由 `Link`；收起态标签只能 `sr-only` + `Tooltip`，不能显示小字标签；分组没有 `role="group"`，收起后没有分隔线；底部放不了带选中态的导航项；收起按钮名称固定（外壳）。
- **Alert**：没有 `className` 参数（外壳、检测）；没有 tertiary 色调（检测、历史）；只有 `ng` / `fault` 是 `role="alert"`，`warn` / `info` 是 `status`（历史、设备）。
- **StatusBadge**：没有中性色调。默认 `info` 是 primary 容器色，`off` 是 tertiary 容器色，无法表达灯未亮、空闲、未开始；各处只能手写无色调类的 `.qs-badge`（外壳、检测、工作台）。

### 表单与选择

- **Select**（Radix 样式下拉）
  - 可访问的 `combobox` 是触发按钮、没有 value，原生 `select` 被 `aria-hidden`；Playwright `selectOption` 与 `userEvent.selectOptions` 对触发按钮无效（外壳、历史、设备）。
  - 选项 label 只接受 string，不能带状态点或独立 `aria-label`（设备）。
  - 忽略祖先 `<fieldset disabled>`（工作台）。
- **SegmentedControl**：选项 label 只接受 string，不能带状态点或独立 `aria-label`（设备）。
- **NumberField**：给了 `min` / `max` 会在失焦时自动夹紧，值类型是 `number | ''`，无法保持越界输入并显示校验错误；`step` 只接受 number，原先的 `step="any"` 表达不了（外壳、设备）。
- **文件输入**：没有组件（工作台）。
- **Button / IconButton**
  - `Button` 不能渲染为链接，没有 `as` / `asChild` / `href`（全部工作线）。
  - `IconButton` 最小 30 px，会把面板标题行撑高（检测）。
  - 文字颜色只取 `--btn-fg`，没有警告色调；宿主要覆写变量并提高选择器权重（工作台）。
- **Chip**：静态模式不透传 `title` 等原生属性（历史）；只放一段文字，放不下“键 + 值”（工作台）。

### 容器与数据

- **Dialog / Drawer**：没有宽度或尺寸参数，宽度要靠 `className` 加双类名或 ref 设 `--modal-w`；没有初始焦点参数，焦点要在打开后用 `requestAnimationFrame` 移到第一个输入框（外壳、工作台、历史）。
- **Card**：只能渲染 `div`，没有 `section` / `region` 语义（历史）；标题槽位外包一层无类名 `div`，面板标题字号要靠宿主 CSS（外壳）；`title` 与 `actions` 不在同一父级，标题加操作按钮要用 `region` 查询（设备）。
- **Table**
  - 不支持行级属性：`onClick`、`className`、`aria-label`、`tabIndex`、`aria-selected`、行级 `title`（历史、设备、工作台）。历史列表因此改成外层容器点击代理加首列按钮，行语义色靠 `data-tone` 与 `:has()`。
  - 不支持 `colSpan`、行头（`th scope="row"`）和可编辑单元格（工作台）。
  - `loading` 整体替换行，无法保留上一次结果并标记加载中；`aria-busy` 只能放在外层容器（历史）。
  - 列不支持固定 `nowrap`（设备）。
- **Pagination**：总是渲染每页条数选择，页大小固定的页面只能用 CSS 隐藏（历史、设备）。
- **Accordion**：没有 `forceMount`，折叠时卸载内容；放在 `<fieldset disabled>` 内时触发按钮被禁用（工作台、设备）。

### 业务向组件

- **VerdictCard**：大字与标题固定，`warn` 只能显示 WARN / 需要复核，与 ui-shell §4 冲突（局部超差须显示大字 OK、副标题“局部超差”）；无法显示 NG 细分副标题；头部只能放 label 和 `cycleId`，不能显示 SN、PLC 码和故障码（端到端脚本依赖“判定结果 · SN …”）；running 状态的描述固定；`state=fault` 优先，会在 FAULT 阶段覆盖本件已有的有效判定（检测）。
- **MeasurementReading / MeasurementReadings**：状态标签面向测量值，用于产量和 OK / NG / ERR 计数会重复标签，0 件 NG 也会标红；没有计数 / KPI 类组件（检测）。值为空时显示未测状态文字，不是占位横线（历史）；不覆盖空格千分位和文字值（设备）。
- **CameraThumbnail / CameraFrameState**：`waiting` / `skipped` 用文字占位替换预览；状态文案固定，无法表达局部超差、断胶、未示教；预览高度固定 88 px；只接受 `src` 或只读 `preview`，放不了实时 canvas（检测、设备）。
- **Tooltip / Popover**：`Tooltip` 不支持点击后固定展开；`Popover` 不能悬停 / 聚焦打开，角色是 dialog（检测）。

## 四、遗留与后续

- `Modal.tsx` 的 `side` 参数（抽屉形态）没有调用方，对应的样式已删；参数本身留在 TSX 里，下次动 `Modal` 时一并清理。
- PLC 点位编辑、写值、地址表 JSON、S7 一期点表对话框和相机未保存提示仍走 `Modal.tsx`，没有直接换成 `Dialog`。
- 缺口清单中影响最大的是：`Button` 渲染为链接、`Table` 行级属性、`StatusBadge` 中性色调、`Select` 的可测试性（combobox 带 value）、`VerdictCard` 的业务字段。向 qing-steel 提需求时建议优先这几项。
