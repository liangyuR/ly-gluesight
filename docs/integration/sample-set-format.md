# 离线样本集格式 v1（契约，后端与转换脚本共同遵守）

样本集 = 一个目录，根下 `sample-set.json` + `parts/<partId>/` 图像。原图不入仓库。

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
- cameraId：样本集 `cameras[].id`，如 `cam1`。
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
  "mmPerPx": null,
  "cameras": [ { "id": "cam1", "views": [1] } ],
  "shots": [
    { "shotId": "S001", "cameraId": "cam1", "views": [1], "origin": "Frame1000005.jpg" }
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
- `mmPerPx`：已知像素当量填数字，未知填 null（示教时人工填写）。
- `shots[].origin` / `parts[].origin`：原始文件/目录名，仅供追溯显示。
- `label`：`ok` / `ng` / `unknown`；`shotLabels` 可选，值为原始标签小写（如 `ok`、`breaking`），缺省视为未标注。
- 标签只用于展示"期望 / 实际"和测试断言，**不参与判定**。
- 每个 part 应包含 shots × views 的全部文件；缺失文件合法（表示该件该点缺图），加载时报告缺失清单，运行时该帧按缺帧处理。
- `sn` 为字符串（可能含字母）；PLC 的 partSn 是 u32，离线产线另行分配数值 partSn，并记录 partSn ↔ partId 的对应。

## 转换与校验工具
`scripts/sample-set/convert.py`（Python 3.11+ 标准库）把现场原图转换为本格式，并提供 `validate` 子命令按本契约检查样本集：

```
python scripts/sample-set/convert.py isv  <ISV源目录>  <输出目录> [--shots 1-8] [--id ID] [--name 名称] [--force]
python scripts/sample-set/convert.py mx11 <MX11源目录> <输出目录> [--shots 36-43] [--id ID] [--name 名称] [--force]
python scripts/sample-set/convert.py validate <样本集目录> [--strict]
```
- `--shots` 按排序后的拍照点序号选子集，生成的 shotId 从 S001 连续重新编号，`shots[].origin` 保留原始来源。
- 使用 `--shots` 时，`label` 与 `shotLabels` 只按所选拍照点计算。
- `validate` 退出码：0 通过；1 格式错误；2 仅缺图且指定了 `--strict`。
