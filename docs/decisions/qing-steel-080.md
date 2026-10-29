# qing-steel React 组件使用约定

当前依赖 qing-steel 0.10.0，样式入口为 `qing-steel/react.css`；应用使用 `ThemeProvider` 和 `SnackbarProvider`。通用交互优先使用组件库，业务图像叠加、胶路几何和采集时序保留领域实现。组件能力以锁定版本的公开类型和实际行为为准。

## 样式与测试

- `main.tsx` 在 feature CSS 后加载组件库样式；局部布局覆盖使用父级限定或双类名。不要复制组件库的按钮、选择器、焦点和动效样式。
- `Button asChild` 可以包裹路由 `Link`，保持链接语义和客户端路由；纯链接也可以使用 `href`。
- `Select` 使用 Radix 弹层，测试通过 combobox 打开、option 选择；不要把 `toHaveValue` 或 `selectOption` 施加到按钮式 combobox。弹层挂在主题容器内，应从页面查找选项。
- 使用 `useSnackbar` 的测试需要 `SnackbarProvider`；Radix 控件测试需要项目测试设置中的浏览器环境桩。
- 数据保存、字段禁用、越界校验、折叠状态与路由语义属于业务契约，换组件时须保留。

## 配方控件映射

| 场景 | 组件与约定 |
| --- | --- |
| 操作、跳转、图标按钮 | `Button`、`Button asChild`、`IconButton`；危险或警告操作使用 `danger` / `tone` |
| 普通输入、数字、选择、勾选 | `TextField`、`NumberField`、`Select`、`Checkbox`、`Switch` |
| 允许越界输入并提示错误的数字参数 | `NumberField clamp={false}`，空值保持未填写语义；允许任意精度时用 `step="any"` |
| 文件选择与拖入 | `FileInput`；保留格式检查、读入失败提示及选同一文件重试的行为 |
| 状态与提示 | `StatusBadge`（含 `neutral`）、`Alert`、`Chip` |
| 卡片与折叠详情 | `Card`、`Accordion`；需要保留内容状态时 `forceMount`，冻结参数仍可展开时 `ignoreFieldsetDisabled` |
| 段内选项切换 | `Tabs` 或 `SegmentedControl`，不另写点击和选中动效 |
| 参数表 | `Table` 的 `render`、`rowHeader`、`colSpan`、`getRowProps`，不因可编辑单元格而另建通用表格 |
| 帮助提示 | `Tooltip pinnable`，需要内容布局时 `Popover` |
| 对话框 | `Dialog` / `Drawer` 的 `size`、`width`、`initialFocus` |

段列表使用 `Tabs`，只显示段名称（如“段 1 · 直线”），按内容宽度排列，不拉伸填满容器；多段超出宽度时横向滚动。选中标记、悬停、按下、焦点与方向键沿用组件库。标定状态和检测相机保留在所选段参数详情中。

## 组件库已提供的扩展能力

设置页使用 `Select` / `NumberField` 的 `label` 与 `description` 展示字段及帮助，目录入口使用 `Button variant="text"`，仅保留长路径换行和键值布局。各分区独立保存使用 `tonal` 与 `loading`，不在同屏放置多个主操作。

项目页使用 `Table`、`Menu`、`Dialog` 与组件库表单；输入型对话框通过 `initialFocus="first-field"` 聚焦首个字段，异步操作使用 `loading`。当前项目禁删原因使用可键盘聚焦的 `Tooltip`，统计失败使用 `Tooltip pinnable` 显示完整原因。静态项目统计与键值说明保留语义布局，业务 CSS 不另画控件状态。

通讯页使用 `Card` 承载状态条和配置分组，提示折叠使用 `Accordion`，心跳信号使用 `StatusBadge` 与 `Tooltip`。新增点位按钮独立于折叠标题，避免嵌套交互。通讯弹窗复用 `Dialog` 的 `width`、`initialFocus` 和关闭锁；日志刷新、导出使用组件的 `loading`，日志表格使用内置加载状态。PLC 阶梯趋势图属于领域绘图，保留采样和时间映射，空态使用 `EmptyState`。生产操作锁、受保护点位和草稿保存规则继续由业务层负责。

0.10.0 的公开接口包含下列能力。仍使用本地实现的调用方应按实际业务行为迁移，不能再以这些能力缺失为保留理由。

- `NavigationDrawer`：`renderLink`、`collapsedLabels`、`footerItems`、自定义展开与折叠标签。
- `SnackbarProvider`：`placement`、`container`、`max`。
- `Alert`：原生属性、`className`、`role`、`tertiary` 色调。
- `Select` / `SegmentedControl`：ReactNode 选项、`textValue`、选项可访问名称。
- `NumberField`：`clamp`、`rangeError`、`step="any"`。
- `Card`：继承 `Surface` 的 `as` 及原生属性。
- `Table`：行属性、行色调、行头、跨列、加载时保留数据、列不换行；`Pagination` 支持隐藏每页数量选择。
- `Chip`：ReactNode 内容及原生属性；`IconButton` 支持 `xs`。
- `VerdictCard`：结果代码和标题、状态内容、故障时保留结果、头部序列号与自定义元信息。
- `CameraThumbnail` / `CameraStrip`：自定义预览、预览尺寸与状态文字、保留预览。
- `MeasurementReading`：文本值、分组格式、待测标签；计数使用 `StatCounter` / `KpiGroup`。
- `Tooltip`：可固定展开；`Popover`：悬停打开和 tooltip 角色。

## 领域视图与路由步骤

`GrayViewer`、胶路地图、走廊几何编辑、采集帧组与段边界时间轴属于领域视图。其数据映射、canvas/SVG 绘制和指针操作保留本地；外围通用控件使用组件库。

配方路由步骤导航使用 qing-steel 的 `Steps`，由 `renderLink` 接入 React Router `Link`，当前路径提供 `currentValue`，保留 `aria-current="step"`。阶段编号、完成状态、等待示教件提示与当前示教完成条数由应用传入；完成条件继续使用 `stageStatusV2`，组件库不推断流程或禁止跳转。

普通键值内容使用语义布局即可；不应为了覆盖所有 DOM 元素而将静态文字或业务绘图强行包装为交互组件。
