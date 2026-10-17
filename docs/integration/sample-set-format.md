# 离线样本集格式 v1（契约，后端与转换脚本共同遵守）

样本集 = 一个目录，根下 `sample-set.json` + `parts/<partId>/` 图像。原图不入仓库。

样本集目录及其各级子目录（含 `parts/`、`parts/<partId>/`）和图像文件都不能是 junction、符号链接或 OneDrive 等同步盘的 reparse point，否则后端拒绝加载；样本集应放在本地非同步目录。

```
<set>/
  sample-set.json
  parts/
    p01/
      S001_cam1_v1.jpg
      S002_cam1_v1.jpg
      ...
    p02/...
```

## 图像命名（唯一规则）
`parts/<partId>/<shotId>_<cameraId>_v<view>.<ext>`
- shotId：`S` + 三位序号（S001 起），对应配方拍照点 ID；序号 = 样本集 `shots[]` 中的顺序（从 1 开始）。
- cameraId：样本集 `cameras[].id`，如 `cam1`；只能用 ASCII 字母、数字、`-`、`_`，长度 1–32。
- partId：同 cameraId 字符规则；重复按不区分大小写判断（`p01` 与 `P01` 视为重复）。
- view：1..3。
- ext：jpg/png/bmp/tif/pgm，保持原格式，不转码。
- 业务关联只用 partId/shotId/cameraId/view，不使用任何内容哈希。

## sample-set.json
```json
{
  "schemaVersion": 1,
  "id": "isv-20261009",
  "name": "ISV 现场原图（20 件）",
  "source": "ISV数据（2026-10-09 现场采集）",
  "imageSize": [1280, 960],
  "mmPerPx": 0.1,
  "cameras": [ { "id": "cam1", "views": [1] } ],
  "shots": [
    { "shotId": "S001", "cameraId": "cam1", "views": [1], "origin": "Frame1000005.jpg" },
    { "shotId": "S002", "cameraId": "cam1", "views": [1], "origin": "Frame1000006.jpg", "mmPerPx": 0.085 }
  ],
  "parts": [
    {
      "partId": "p01",
      "sn": "02270108TA",
      "label": "unknown",
      "shotLabels": { "S047": "breaking" },
      "capturedAt": "2026-10-09T19:21:51+08:00",
      "origin": "20261009192151_02270108TA"
    }
  ]
}
```
- `schemaVersion` 只接受 1；未知版本直接拒绝。
- `mmPerPx`：全局像素当量（mm/px），已知填正的有限数，未知填 null（示教时人工填写）。
- `shots[].mmPerPx`（可选）：该拍照点的真实像素当量，覆盖全局 `mmPerPx`；取值为正的有限数，或省略/null（回落全局）。非正、非有限值整份样本集拒绝加载，错误中带 shotId。
- 某拍照点的有效当量 = 该点 `mmPerPx` ?? 全局 `mmPerPx`；两者都没有时视为未声明。
- 工作台整圈采集使用样本集时，以样本集声明的有效当量作为该拍照点的像素当量与标定验证依据；用户在工作台改动该点当量后，该点回到人工标定验证。未声明当量的拍照点照常走人工标定。
- `imageSize`：全部样本图的宽高都必须与之一致；`validate` 读取每张图的头部（jpg/png/bmp/pgm/tif），尺寸不符或头部无法解析报格式错误；运行时读到尺寸不符的图计入缺图。
- `shots[].views` 必须与所属相机 `cameras[].views` 完全相等（不能只取子集）。
- `shots[].origin` / `parts[].origin`：原始文件/目录名，仅供追溯显示。
- `label`：`ok` / `ng` / `unknown`；`shotLabels` 可选，值为原始标签小写（如 `ok`、`breaking`），缺省视为未标注。
- 标签只用于展示"期望 / 实际"和测试断言，**不参与判定**。
- 每个 part 应包含 shots × views 的全部文件；缺失文件合法（表示该件该点缺图），加载时报告缺失清单，运行时该帧按缺帧处理。
- `sn` 为去除空白后非空的字符串（可能含字母）。PLC 的 partSn 自协议 3 起是 S7 STRING[24]（1–24 个 ASCII 可打印字符，原样保存与回显）；离线产线为每件分配形如 `OFFLINE-0001234567`（`OFFLINE-` 加 10 位数字）的 SN，同一离线运行内唯一，按该 SN 字符串认领样本件，并记录 SN ↔ partId 的对应，样本集自身的 `sn` 仅作追溯显示。

## 转换与校验工具
`scripts/sample-set/convert.py`（Python 3.11+ 标准库）把现场原图转换为本格式，并提供 `validate` 子命令按本契约检查样本集：

```
python scripts/sample-set/convert.py isv  <ISV源目录>  <输出目录> [--shots 1-8] [--id ID] [--name 名称] [--force] [--mm-per-px 0.1] [--shot-mm-per-px S001=0.085,S002=0.09]
python scripts/sample-set/convert.py mx11 <MX11源目录> <输出目录> [--shots 36-43] [--id ID] [--name 名称] [--force] [--mm-per-px 0.1] [--shot-mm-per-px S001=0.085]
python scripts/sample-set/convert.py round <单圈源目录> <输出目录> [--id ID] [--name 名称] [--force] [--mm-per-px 0.1]
python scripts/sample-set/convert.py validate <样本集目录> [--strict]
```
- `round`：源目录平铺一圈手动采集图 `<点位>_<相机序号>.<ext>`（如 `10_3.bmp`），生成 1 件样本（sn 为源目录名）。点位与相机序号都按数值排序；相机序号 n 对应 cameraId `camn`；拍照点按“点位 → 相机”顺序编号（S001 = 点位 1 相机 1，S002 = 点位 1 相机 2 …），每台相机的第 k 个拍照点即第 k 个点位。某点位缺某相机的图时仍保留该拍照点并按缺图报告，不错位。
- `--shots` 按排序后的拍照点序号选子集，生成的 shotId 从 S001 连续重新编号，`shots[].origin` 保留原始来源。
- 使用 `--shots` 时，`label` 与 `shotLabels` 只按所选拍照点计算。
- `--mm-per-px` 写入全局 `mmPerPx`；`--shot-mm-per-px` 写入对应 `shots[].mmPerPx`，shotId 指生成后的编号（受 `--shots` 重新编号影响）。值必须为正的有限数，shotId 必须是生成清单中存在的拍照点，否则报错退出且不写输出。
- `--force` 只覆盖顶层仅含 `sample-set.json` 文件与 `parts/` 目录、且 `validate` 无格式错误（缺图可以）的样本集目录；其他非空目录一律拒绝删除并原样保留。
- `validate` 退出码：0 通过；1 格式错误；2 仅缺图且指定了 `--strict`。
