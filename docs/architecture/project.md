# 项目（Project）设计

## 一、目标

把“一套可上线的检测方案”收拢成一个**项目**：相机组参数、PLC 配置、节拍设置、工位标定、配方（候选、示教资料、采集轮次、标定轮次、发布包）以及这套方案产生的检测记录与历史原图，都归属于某个项目。软件启动时打开当前项目，操作员可以切换、新建、删除项目。

典型用途：同一台工控机在换线、换夹具或换相机布置后切到另一套方案；调试新方案时不影响已上线的方案。

不在本设计范围：PLC 不感知项目；不支持两个项目同时运行；不做按项目的操作权限。

## 二、概念与边界

- **项目** = 一个工位的完整检测方案及其检测历史。一个项目内可以有多个配方，PLC 仍以 `productCode` 在**当前项目**的配方中选取；当前项目没有对应配方时，按现有“配方不存在”的结果码拒绝。
- **本机资料**指与项目无关、属于这台工控机或整个资料目录的内容：PLC 握手事务、日志、站点模式、受管样本、界面偏好等。
- 任一时刻只有一个**当前项目**；所有读写项目资料的代码只通过当前项目根目录访问。唯一例外是项目管理页读取其他项目的统计信息（只读）和删除其他项目。

### 归属划分

| 资料 | 现位置 | 归属 | 说明 |
| --- | --- | --- | --- |
| 相机组 | `config/cameras.json` | 项目 | 相机编号、序列号、触发、帧率；项目之间可用不同的相机组 |
| PLC 配置 | `config/plc.json` | 项目 | 地址、点表；切换后按新配置重连 |
| 节拍设置 | `config/cycle.json` | 项目 | 含历史保留天数 |
| 飞拍估算参数 | `config/feasibility.json` | 项目 | |
| 工位标定 | `calib/` | 项目 | 含 `calib/device/` |
| 生产配方 | `recipes/` | 项目 | 计划版本计数不在这里，见第五节 |
| 候选与示教资料 | `workspaces/` | 项目 | 含 `comparisons/` |
| 发布包 | `vision/releases/` | 项目 | |
| 整圈采集轮次 | `recipe-capture/` | 项目 | |
| 检测记录库 | `inspection.db` | 项目 | 每个项目一份，表结构不变（含 `recipe_snapshots`、`workspace_instances`） |
| 历史原图 | `records/`、`records-offline/` | 项目 | |
| 审计 spool | `audit-spool/` | 项目 | 待入库的件证据，随记录库走；切换前必须清空 |
| PLC / 示教握手事务 | `state/` | 本机 | 绑定物理 PLC，与项目无关，见第四节 |
| 握手审计、日志 | `state/plc-handshake*`、`logs/` | 本机 | 日志条目带项目编号 |
| 站点模式、离线选择 | `config/site-offline.json` | 本机 | |
| 受管样本 | `samples/` | 本机 | 按样本集 ID 引用，各项目共用 |
| 界面偏好 | `config/preferences-<SID>.json` | 本机（按账号） | `lastWorkspace` 改为按项目分别记 |
| 缓存、WebView、导出、迁移 | `cache/` 等 | 本机 | |

## 三、目录结构

```text
%ProgramData%\ly\GlueSight[-dev]\
  config/
    current-project.json        { "schema": 1, "current": "<projectId>" }
    site-offline.json  preferences-<SID>.json
  projects/
    <projectId>/
      project.json              项目身份与描述
      config/                   cameras.json  plc.json  cycle.json  feasibility.json
      calib/
      recipes/
      workspaces/
      vision/releases/
      recipe-capture/
      inspection.db
      records/  records-offline/
      audit-spool/
    .trash/                     删除中的项目，启动时清理
  state/  samples/  logs/  cache/  webview/  exports/  migration/
```

`project.json`：

```json
{
  "schema": 1,
  "id": "p-20261024-01",
  "name": "京航 A 线",
  "description": "",
  "createdAt": 1793030400000,
  "createdBy": "<软件版本>",
  "copiedFrom": null
}
```

- `id` 创建时分配、之后不变，只含 `[A-Za-z0-9_-]`，同时是目录名；`name` 可改，在本机内不重名。
- 项目内路径引用的写法不变（`workspace.json` 的 `calibTag.path` 等仍为绝对路径）；以后复制、导出项目时用 `storage/relocate.rs` 重写。

## 四、启动与切换

### 启动

1. 资料目录就绪检查照旧（`storage_startup`）。
2. 读 `config/current-project.json`，校验 `projects/<id>/project.json` 存在且格式可读；清理 `projects/.trash/` 下的残留。
3. 把当前项目根目录放进托管状态 `ActiveProject`，之后记录库、`CameraRig`、`PlcHost`、`CycleHost`、`WorkspaceHost` 等按它取路径初始化。
4. 本机还没有任何项目时，自动新建空的“默认项目”（编号 `default`）并设为当前项目。
5. 已有项目但当前项目无效（指针缺失、目录被删、`project.json` 损坏）时**不自动挑一个项目**：资料准备窗口进入“选择 / 新建项目”状态，检测服务、相机、PLC 均不启动，选定后写指针并重启。只有一个可用项目时也要求确认一次。

### 切换

切换采用**写指针后重启进程**：各宿主都在启动时一次性按路径初始化，开发机实测重启式切换约 1.1 秒，换线场景可以接受。不做不重启的热切换。

1. 操作员在侧栏的项目菜单或项目管理页选目标项目，确认框显示目标项目名称、编号、配方数与检测记录数，并提示“将重启软件”；候选配置有未保存编辑或保存失败时不能切换。
2. 后端 `project_switch(id)` 检查前置条件，任一不满足即拒绝并给出原因：
   - 检测未布防，没有在途工件，没有未结 PLC 事务（与站点模式切换共用 `site::check_switch`）；
   - 没有进行中的整圈采集、示教采集或示教握手；
   - 录制队列与审计 spool 已写完；
   - 前端没有未保存的候选编辑（前端自查后才发请求）。
3. 原子写 `current-project.json`，写一条 info 日志（旧项目 → 新项目），然后 `app.restart()`。
4. 重启后按“启动”流程打开新项目。

### PLC 握手与项目无关

PLC 握手只关心工件，不记录、不核对项目，握手状态与协议都不改。正常切换要求没有未结事务，所以事务不会跨项目。唯一例外：软件异常退出时 PLC 还有未结事务，下次启动又因当前项目无效、在选择页换了项目，这时“中断工件”按现有逻辑补记到新打开的项目。这是已接受的取舍。

## 五、跨项目约束

- **计划版本全局单调**：计划版本是配方计划的身份，随检测记录追溯；换项目也不能复用用过的号。计数放在本机 `state/plan-version`，所有项目共用一个递增计数（PLC 只发型号，不收计划版本）。
- **记录与历史按项目隔离**：每个项目一份 `inspection.db`、`records/`、`records-offline/`，历史页只显示当前项目的记录，复测、重判自然只用当前项目的配方。记录库表结构与版本不变。
- **历史清理**：只清理当前项目的过期记录，保留天数取当前项目的 `cycle.json`。非当前项目不再产生新记录；磁盘空间不足时由操作员在项目管理页删除不用的项目。
- **审计与诊断**：`applog` 字段增加 `projectId`；诊断包导出当前项目的 `config/` 与 `project.json`。
- **相机与 PLC 物理独占**：不同项目可以引用同一台相机或同一个 PLC 地址，因为同时只运行一个项目，不做跨项目冲突检查。
- **配方迁移包**（`workspace/transfer.rs`）照旧在项目内导入导出；导入目标是当前项目，编号冲突只与当前项目比较。

## 六、项目管理页

入口：侧栏项目卡片菜单的“管理项目…”与导航里的“项目”页（`/projects`）。页面列出本机全部项目，提供新建、重命名、切换、复制、导出、导入、删除。

### 列表

每个项目一行：名称、编号、“当前”标记、描述、创建时间、配方数、检测记录数与时间范围、占用空间。占用空间与记录数可能较慢，单独异步计算，算完前显示“计算中”。`project.json` 损坏的目录也列出，标“不可用”并显示原因，只允许删除。

### 新建

| 字段 | 规则 |
| --- | --- |
| 名称 | 必填，本机内不重名 |
| 编号 | 默认按日期生成 `p-YYYYMMDD-NN`，可改；只含 `[A-Za-z0-9_-]`，不与已有目录（含 `.trash`）重名，创建后不可改 |
| 描述 | 可选 |
| 初始内容 | “空项目”或“复制当前项目的硬件配置”（`config/` 与 `calib/`：相机组、PLC、节拍、飞拍估算参数、标定）；不复制配方、候选、采集轮次与记录 |
| 创建后切换 | 默认不勾；勾选时按切换流程检查并重启 |

### 重命名

只改 `project.json` 的名称和描述，不影响编号与目录。

### 删除

- 当前项目不能删除，按钮置灰并提示“先切换到其他项目”。
- 确认框列出将被永久删除的内容：配方数、候选数、采集轮次数、检测记录数及时间范围、占用空间，要求输入项目名称后才能确认。
- 删除先把目录原子改名到 `projects/.trash/<id>.<毫秒时间戳>`，列表立即不再出现该项目，再在后台删除；中途退出时下次启动继续清理。删除失败（文件被占用等）写日志，残留留在 `.trash` 等下次启动清理。

### 导出与导入

- **导出**：任一可用项目都能导出为 ZIP（`project/transfer.rs`，格式版本 1）。包内是 `project/` 下的 `project.json`、`config/`、`calib/`、`recipes/`（含修订来源记录）、`workspaces/`、`vision/`、`recipe-capture/`；勾选“包含检测记录与历史原图”时另含 `inspection.db`（用 `VACUUM INTO` 做含 WAL 的一致性快照）、`records/`、`records-offline/`，默认不含。`audit-spool/` 不导出。`manifest.json` 记格式版本、来源项目、来源项目目录、导出时间、来源软件版本、是否含记录及每个文件的字节数与 SHA-256。导出当前项目前按切换的条件检查：没有在途件与未结事务、没有在采集、录制与追溯已写完。
- **导入**：先完整解到 `cache/project-import-*` 暂存目录，格式版本不认识、路径不在白名单结构内（含 `..`）、清单外条目、重复条目、字节数或校验和不符都整包拒绝，暂存目录用后删除，不留下半个项目。导入为新项目，编号与名称按新建项目的规则校验；`project.json` 的 `copiedFrom` 记来源项目编号。
- **路径**：`storage/relocate.rs` 把 `workspace.json`、采集轮次 `round.json` 与记录库原图目录中指向来源项目目录的绝对路径改写到新项目目录，再核对没有残留的来源路径。
- **计划版本**：生产配方保持可生产，发布包一起导入；配方文件、`.revision-sources` 与（含记录时）记录库配方快照中用到的计划版本，按本机 `state/plan-version` 整体换成新号（同一旧号换成同一新号），修订号不变。发布包与发布归档里冻结的配方文档保留原号，它们只按修订号与生产配方对应，不发给 PLC。
- **候选**：回到草稿，清掉待生效版本与验证结论；没带检测记录时去掉来自检测历史的验证样本。
- 导入完成后提示核对相机组、PLC 配置与标定；切换到该项目仍按切换流程重启。

### 复制

- 入口：列表中任一可用项目一行的“复制”。填新项目名称（默认“<原名>（副本）”）、编号、描述，可选“包含检测记录与历史原图”（默认不含）。
- 内容与导出相同（含记录时记录库同样用 `VACUUM INTO` 做快照，`audit-spool/` 不复制）；文件先拷进 `cache/project-copy-*` 暂存目录，再按导入的规则落成新项目：路径改写到新项目目录、计划版本按本机重新分配、候选回到草稿，最后原子移入 `projects/<新编号>/`。来源项目不改动；`copiedFrom` 记来源编号。
- 复制当前项目前按导出的条件检查。复制后不自动切换，完成框里可直接进入切换流程。

## 七、旧资料不迁移

旧资料全部抛弃、不迁移：新版本不读取根目录下的 `config/cameras.json`、`plc.json`、`cycle.json`、`feasibility.json`、`calib/`、`recipes/`、`workspaces/`、`vision/releases/`、`recipe-capture/`、`inspection.db`、`records/`、`records-offline/`、`audit-spool/`，首次启动新建空的默认项目。软件不自动删除这些旧文件，启动时检测到它们就写一条提示日志，由人工清理。旧 `recipes/.plan-version` 也不读取，`state/plan-version` 从头开始计数。

## 八、代码改动点

| 范围 | 改动 |
| --- | --- |
| `storage.rs` | 新增 `projects_root`、`project(app)`（当前项目根）、`project_config(app)`；`config(app)` 只留本机配置 |
| 新增 `project.rs` | `ProjectMeta`、`ActiveProject` 托管状态、`current-project.json` 读写、列表、启动解析、`project_switch` |
| 新增 `project/transfer.rs` | 项目导出包的打包、校验、导入（路径改写、计划版本重新分配、候选回到草稿）；复制项目复用同一落地流程 |
| 新增 `project/manage.rs` | 新建（空 / 复制硬件配置）、重命名、删除（`.trash` 改名与后台清理）、项目统计（只读打开其他项目的记录库、统计目录大小） |
| `lib.rs` | `ActiveProject` 在记录库之前解析，`open_store` 改用项目路径；无效时只启动选择项目所需的命令 |
| 路径调用方 | `camera.rs`（`cameras.json`、`recipe-capture`）、`plc.rs`（`plc.json`）、`cycle.rs`（`cycle.json`、`recipes`、`records`、`records-offline`、`audit-spool`）、`vision.rs`（`calib`）、`workspace.rs` 与 `workspace/transfer.rs`（`workspaces`、`vision/releases`、`recipe-capture`）、`diagnostics.rs`、`storage.rs` 的飞拍估算参数 |
| `recipe.rs` | 计划版本计数放在 `state/plan-version` |
| `site.rs` | `check_switch` 抽成站点模式切换与项目切换共用 |
| 前端 | 侧栏当前项目与切换菜单；项目管理页；启动时的“选择项目”状态；`lastWorkspace` 按项目记 |
| 文档、脚本 | `storage-layout.md` 改为项目目录结构；`docs/README.md` 关键模块表；`scripts/soak/lib.mjs` 的清理目录 |

## 九、已确认

1. 受管样本 `samples/` 属于本机，各项目共用。
2. 切换采用写指针后重启软件，不做热切换。
3. 不做按项目的操作权限控制。
4. 旧资料不迁移，全部抛弃（第七节）。
5. 检测记录、历史原图与审计 spool 放在项目目录下，历史页按项目隔离。
6. 记录库每个项目一份，表结构不变，保留 `recipe_snapshots`、`workspace_instances`。
7. 项目管理页支持新建、重命名、切换、复制、导出、导入与删除。
8. PLC 握手只关心工件，不记录、不核对项目。
9. 计划号不承接旧 `recipes/.plan-version`，新计数从头开始。
