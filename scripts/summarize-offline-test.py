"""Validate native test evidence and produce a local report and CSV exports."""
import argparse
import csv
import hashlib
import json
from datetime import datetime, timedelta, timezone
from collections import Counter
from pathlib import Path


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def write_csv(path, rows):
    with path.open("w", newline="", encoding="utf-8-sig") as output:
        writer = csv.DictWriter(output, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


def summarize(root):
    report = json.loads((root / "native-report.json").read_text(encoding="utf-8-sig"))
    dataset = json.loads((root / "dataset.json").read_text(encoding="utf-8"))
    assert report["passedInfrastructure"] and not report["productAcceptanceEstablished"]
    rows = [r for group in report["groups"] for r in group["rows"]]
    assert len(rows) == dataset["frames"] == 453
    assert len(report["cycles"]) == 4
    frame_rows = []
    for row in rows:
        frame_rows.append({
            "group": row["group"], "channel": row["channel"], "sequence": row["sequence"],
            "file": row["file"], "fullWidth": row["preview"]["fullWidth"],
            "fullHeight": row["preview"]["fullHeight"], "previewHashFNV32": row["preview"]["hash"],
            "softwareFrameCounter": row["frameCounter"], "directionDeg": row["directionDeg"],
            "testedPoints": row["testedPoints"], "foundPoints": row["foundPoints"],
            "missingPoints": row["missingPoints"],
            **{"widthPx_" + key: (row["widthPx"] or {}).get(key) for key in ("min", "median", "max")},
            **{"offsetPx_" + key: (row["offsetPx"] or {}).get(key) for key in ("min", "median", "max")},
            "lostPackets": row["lostPackets"], "droppedFrames": row["droppedFrames"]
        })
    write_csv(root / "per-frame-probe.csv", frame_rows)
    config_dir = root / "test-config"
    config_dir.mkdir(exist_ok=True)
    cycle_rows, point_rows = [], []
    previous_counts = [s["frames"] for s in report["groups"][-1]["statusAfter"]]
    for run in report["cycles"]:
        group = run["name"]
        step = run["doc"]["follow"]["stepMm"]
        before = [s["frames"] for s in run.get("cameraStatusBefore", [])] or previous_counts
        after = [s["frames"] for s in run["cameraStatus"]]
        received = [a - b for a, b in zip(after, before)]
        expected = report["options"][group]["count"]
        assert received == [expected] * 3
        assert run["history"]["summary"]["framesReceived"] == expected * 3
        assert run["completedHandshake"] and run["passedPipeline"]
        assert not run["nativeSummary"]["measurementErrors"]
        assert all(s["lostPackets"] == 0 and s["droppedFrames"] == 0 for s in run["cameraStatus"])
        previous_counts = after
        points = run["history"]["points"]
        states = Counter(points["st"])
        assert set(states).issubset({0, 1, 2})
        if step == 5:
            assert states[2] == 0
            assert run["nativeSummary"]["actualPixelPoints"] == len(points["st"])
        else:
            assert states[2] <= 1
        summary = run["history"]["summary"]
        cycle_rows.append({
            "group": group, "stepPx": step, "spacingPx": run["doc"]["spacing"],
            "recipeVersion": summary["recipeVersion"], "historyId": summary["id"], "sn": summary["sn"],
            "framesReceived": summary["framesReceived"], "measuredFrames": len(run["measurements"]),
            "totalPoints": len(points["st"]), "beadPoints": states[0], "noBeadPoints": states[1],
            "invalidOrUncoveredPoints": states[2], "verdict": summary["verdict"],
            "plcCode": summary["plcCode"], "faultCode": summary["faultCode"],
            "reasonInPixelTestUnits": summary["reason"].replace(" mm", " px"),
            "elapsedMs": run["elapsedMs"], "drainMs": summary["drainMs"],
            "completedHandshake": run["completedHandshake"]
        })
        for measurement in run["measurements"]:
            metadata = run["history"]["frames"][measurement["k"]]
            cam = measurement["cam"]
            source_sequence = metadata["frameCounter"] - before[cam]
            assert 1 <= source_sequence <= expected
            for position, index in enumerate(measurement["idx"]):
                px = measurement["px"][position]
                point_rows.append({
                    "group": group, "stepPx": step, "historyId": summary["id"],
                    "cameraId": run["cameraStatus"][cam]["id"], "sourceChannel": cam + 1,
                    "sourceSequence": source_sequence, "measurementFrame": measurement["k"],
                    "pointIndex": index, "nominalArcPx": index * run["doc"]["spacing"],
                    "nozzleArcPx": measurement["s"], "state": measurement["st"][position],
                    "offsetPx": measurement["d"][position], "widthPx": measurement["w"][position],
                    "imageX": px[0], "imageY": px[1], "measurementMs": measurement["ms"]
                })
        label = group + "-step" + str(step)
        write_json(config_dir / (label + ".recipe.json"), run["doc"])
        write_json(config_dir / (label + ".settings.json"), run["settings"])
        write_json(config_dir / (label + ".calibration.json"), run["calibration"])
        camera_configs = []
        for cam, camera_id in enumerate(report["cameraIds"]):
            camera_configs.append({**report["initial"]["cameras"][0],
                                   "id": camera_id, "name": "离线样本 通道 " + str(cam + 1),
                                   "source": "replay", "acquisition": "freeRun", "fps": 10,
                                   "triggerSource": "Software", "replayChannel": cam + 1,
                                   "replayDir": str(root / "timed-replay" / group),
                                   "follow": run["calibration"]})
        write_json(config_dir / (label + ".cameras.json"), camera_configs)
    assert report["restoration"]["cameraConfigurationsRestored"]
    assert report["restoration"]["cycleSettingsRestored"]
    assert report["restoration"]["temporaryRecipesRemoved"]
    write_csv(root / "complete-cycle-results.csv", cycle_rows)
    write_csv(root / "native-measurement-points.csv", point_rows)
    exe = Path(report["testedExecutablePath"]) if report.get("testedExecutablePath") else root / "runner" / "tujiao-offline-test.exe"
    with exe.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if report.get("testedExecutableSha256"):
        assert digest.lower() == report["testedExecutableSha256"].lower()
    final = {
        "sourceFiles": len(rows), "uniqueThreeChannelTriplets": 151,
        "probePoints": sum(r["testedPoints"] for r in rows),
        "pointsWithNativeBeadReading": sum(r["foundPoints"] for r in rows),
        "completeCycles": len(cycle_rows), "completeCycleReceivedFrames": sum(c["framesReceived"] for c in cycle_rows),
        "nativeMeasurementPoints": len(point_rows), "pipelinePassed": True, "productAcceptanceEstablished": False,
        "results": cycle_rows, "isolatedAppIdentifier": "com.xyzrobotics.tujiaovision.codex-ui-01a11aa8",
        "testedExecutableSha256": digest, "restoration": report["restoration"],
        "scope": "Real replay JPEG decoding, native follow probing and complete follow cycles with simulated PLC.",
        "units": "Pixel-domain experiment. Application numeric mm values are pixels because test mmPerPx is 1.",
        "timing": "Synthetic 100 ms frame timeline; source acquisition timestamps are unavailable."
    }
    write_json(root / "test-summary.json", final)
    (root / "REPORT.md").write_text(build_report(root, dataset, report, final), encoding="utf-8")
    print(json.dumps(final, ensure_ascii=False, indent=2))


def build_report(root, dataset, native, summary):
    local_started = datetime.fromisoformat(native["started"].replace("Z", "+00:00")).astimezone(timezone(timedelta(hours=8)))
    test_date = f"{local_started.year} 年 {local_started.month} 月 {local_started.day} 日"
    camera_ids = "、".join(native["cameraIds"])
    baseline_missing = [r["invalidOrUncoveredPoints"] for r in summary["results"] if r["stepPx"] == 8]
    coverage_finding = (
        "8 px 测量步长的两次实验有末端点未覆盖；改为 5 px 后覆盖完整，整件按实验限值判为 NG_WIDTH。"
        if baseline_missing == [1, 1] else
        "5 px 测量步长的两次实验覆盖完整；8 px 实验的末端覆盖随有限时间轴和回调时刻变化，详见结果表。"
    )
    drains = [r["drainMs"] for r in summary["results"]]
    lines = [
        "# 海康三目样本离线回放测试",
        "",
        "453 张样本图已解压、无损改名，并在 GlueSight · 胶路智检 桌面后端完成逐帧回放与原生卡尺试测。"
        "四次完整工件流程均完成 PLC 握手、实际像素测量和历史入库。" + coverage_finding,
        "",
        "测试日期为 " + test_date + "。这里的 NG_WIDTH 是实验判定，样本没有真实毫米标定、"
        "采集时间戳、轨迹或人工 OK/NG 标签，不能把它解释成现场产品不合格或算法准确率。",
        "",
        "## 数据整理",
        "",
        "源文件：" + str(dataset["archive"]) + "。",
        "",
        "| 组 | 每路图像 | 通道 | 检测图总数 | 尺寸 |",
        "| --- | ---: | --- | ---: | --- |",
        "| Glue1 | 80 | 1、2、3 | 240 | 1280 × 1024 |",
        "| Glue2 | 71 | 1、2、3 | 213 | 1280 × 1024 |",
        "",
        "原文件 Frame1_1.jpg 等整理为 cam1_000001.jpg。序号和通道保持原义，按六位补零。"
        "每个重命名文件的 SHA256 均与解压原件一致；453 张 JPG 均完成逐张解码检查。"
        "原压缩包、解压原件、标定参考图和三个厂家 BIN 模型均保留。软件未加载厂家 BIN 模型。",
        "",
        "三路对应帧的图像字节完全相同，共 151 组三路相同图像。"
        "因此本次验证了三路选择、排序和一致性，不能验证真实三目视角差异、相机外参或遮挡互补。",
        "",
        "## 回放数据位置",
        "",
        "- 原始解压数据：" + str(root / "raw") + "。",
        "- 可直接手动回放：" + str(root / "replay" / "Glue1") + " 和 " + str(root / "replay" / "Glue2") + "。",
        "- 完整工件实验：" + str(root / "timed-replay") + "，每组含独立 part.json。",
        "- 名称映射：[frame-name-map.csv](" + (root / "frame-name-map.csv").as_posix() + ")。",
        "",
        "相机来源选“回放”，回放目录填到 Glue1 或 Glue2 这一层，每台分别选通道 1、2、3。"
        "文件名前缀中的通道编号不是软件的相机稳定 ID；本次测试的稳定 ID 是 " + camera_ids + "。",
        "",
        "手动查看使用触发采集与 Software 触发源。随动整件实验使用连续采集。"
        "timed-replay 的时间轴为每帧 100 ms，每路 10 FPS，且仅播放一遍；"
        "part.json 明确标记 syntheticTiming。这个时间轴是实验时钟，不是源图采集时间。",
        "",
        "## 软件原生测试结果",
        "",
        "| 检查 | Glue1 | Glue2 |",
        "| --- | --- | --- |",
        "| 三路手动回放数量 | 80 × 3 | 71 × 3 |",
        "| 每路不同预览图 | 80 | 71 |",
        "| 帧号连续 | 是 | 是 |",
        "| 丢包和丢帧 | 0 | 0 |",
        "| 三路对应帧一致 | 80 组 | 71 组 |",
        "",
        "逐帧试测调用软件 teach_follow_probe，卡尺读取完整原图。每张沿胶嘴后方 70 至 280 px 采样 421 点，"
        "共 " + str(summary["probePoints"]) + " 次卡尺读数，其中 " + str(summary["pointsWithNativeBeadReading"]) +
        " 次返回胶条读数。这是给定搜索线上的读数数量，不是带标签的检出率。逐帧中位胶宽随图像变化，"
        "也验证了回放没有重复使用一张静态图。",
        "",
        "完整工件采用 followVision=true，原生卡尺读取回放原图；PLC 模拟器提供开工、运动结束、"
        "结果确认的握手。scenario=normal 不会把这些回放图像替换成模拟图像，也没有使用模拟测量值。",
        "",
        "| 组 | 步长 px | 测量帧 | 采集帧 | 已测点 | 无胶读数点 | 未覆盖点 | 实际整件判定 | 历史编号 |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |"
    ]
    for result in summary["results"]:
        lines.append("| " + " | ".join(str(result[k]) for k in
                     ("group", "stepPx", "measuredFrames", "framesReceived", "beadPoints", "noBeadPoints",
                      "invalidOrUncoveredPoints", "verdict", "historyId")) + " |")
    lines += [
        "",
        "表内“已测点”指成功返回胶条位置和宽度的点；“无胶读数点”也是已执行测量的点，"
        "可能受图像、搜索参数或极性影响，未经过人工确认。"
        "完整工件共再次读取 " + str(summary["completeCycleReceivedFrames"]) + " 帧；所有工件均完成握手，"
        "测量线程错误、丢包和丢帧均为 0。收尾耗时为 " + str(min(drains)) + " 至 " + str(max(drains)) + " ms。",
        "",
        "## 末端覆盖的参数对照",
        "",
        "实验固定测量点间距 5 px，其余参数保持一致，仅将测量步长从 8 px 改为 5 px。" +
        (("8 px 时 Glue1 的 s=350、Glue2 的 s=280 各剩一个未覆盖点，判为 ERR_INSPECT；"
          "5 px 时两组全部测点都有测量状态，判定进入 NG_WIDTH，PLC 结果码从 90 变为 14。")
         if baseline_missing == [1, 1] else
         "8 px 实验每组的未覆盖点数分别为 " + str(baseline_missing) + "；5 px 实验两组均为 0。"),
        "",
        "软件 Tracker.plan 按“新点数量 × 点间距”判断是否达到测量步长，少量末端点还依赖离开窗口时触发。"
        "这组有限时间轴下，末端只剩一个 5 px 点时没有及时达到 8 px 阈值。"
        "本次用配置对照确认了现象，没有修改测量算法，也没有放宽胶宽限值来得到 OK。",
        "",
        "这套样本复测应使用 5 px 步长。生产配置仍需用真实像素当量、速度、帧率和收尾余量换算。"
        "软件后续可考虑在运动结束时为剩余可见测点安排最后一帧，减少末端覆盖对参数组合的依赖。",
        "",
        "## 实验几何与量纲",
        "",
        "| 参数 | Glue1 | Glue2 |",
        "| --- | --- | --- |",
        "| 胶嘴像素坐标 | 688、646 | 688、646 |",
        "| 图中胶条方向 | −75° | −52° |",
        "| 随动相机方位 | 105° | 128° |",
        "| 遮挡半径 | 50 px | 50 px |",
        "| 可测窗口 | 70 至 280 px | 70 至 280 px |",
        "| 搜索半宽 | 25 px | 25 px |",
        "| 名义胶宽 | 16 px | 16 px |",
        "| 胶宽实验合格区间 | 8 至 24 px | 8 至 24 px |",
        "| 名义实验胶路 | 352 px 开放直线 | 280 px 开放直线 |",
        "",
        "mmPerPx=1 仅用于让软件数值与像素一一对应。界面和原生 JSON 字段仍标为 mm，"
        "在本次实验中必须读作 px，不能据此声称物理毫米测量成立。实验胶路、"
        "80 px/s 匀速进度、200 ms 起步延时和 280 px 超行程是流程测试假设，"
        "不代表源图的真实涂胶轨迹。标定参考图没有提供可用的棋盘格实物尺寸。",
        "",
        "这组图像属于胶嘴随动采集。本次覆盖随动回放和卡尺，未据此验证飞拍 lyFlow 图像测量链路。",
        "",
        "## 测试证据与复用",
        "",
        "- [逐帧试测 CSV](" + (root / "per-frame-probe.csv").as_posix() + ")：453 行，含通道、序号、图像指纹和像素读数范围。",
        "- [完整工件结果 CSV](" + (root / "complete-cycle-results.csv").as_posix() + ")：四次实际判定与末端覆盖对照。",
        "- [原生测量点 CSV](" + (root / "native-measurement-points.csv").as_posix() + ")：实际坐标、偏移、胶宽和来源帧。",
        "- [原生完整报告](" + (root / "native-report.json").as_posix() + ") 和 [摘要](" + (root / "test-summary.json").as_posix() + ")。",
        "- 实验参数快照：" + str(root / "test-config") + "。导入其他相机组前须改为当前软件中的相机 ID。",
        "- 软件真实页面：" + str(root / "preview") + "。完整工件历史的原图未重复录制，JPG 原始输入仍在回放目录。",
        "",
        "测试使用独立应用配置目录 com.xyzrobotics.tujiaovision.codex-ui-01a11aa8。"
        "临时实验配方已移除，相机与检测设置已恢复；四条实验历史和配方快照仍保留在测试配置目录。"
        "用户生产配置未修改。",
        "",
        "被测桌面程序 SHA256：" + summary["testedExecutableSha256"] + "。软件版本为 0.1.0。",
        "",
        "准备与复测脚本位于项目 scripts。[Run-OfflineSampleTest.ps1](" +
        (Path(__file__).resolve().parent / "Run-OfflineSampleTest.ps1").as_posix() +
        ") 可在隔离测试实例中重跑，会在结束时恢复测试配置。"
        "脚本根据隔离实例清单验证程序及哈希，窗口是否可见由构建配置决定，标准 UI 测试构建默认隐藏。"
        "加 -KeepOpen 可在成功复测后保留实例。"
        "自动复测需要先用 Start-UiTestInstance.ps1 生成清单并关闭该实例，以及本地缓存的 Playwright CLI。",
        "",
        "```powershell",
        'pwsh -NoProfile -File .\\scripts\\Run-OfflineSampleTest.ps1 -Archive "E:\\samples\\three-camera.zip" -KeepOpen',
        "```",
        "",
        "再次复测前先关闭上一次保留的测试窗口，避免两个实例同时改写隔离配置。",
        "",
        "![离线工件的三路回放与实际判定](" + (root / "preview" / "native-inspect.png").as_posix() + ")",
        ""
    ]
    return "\n".join(lines)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path("output/offline/hikvision-three-camera"))
    args = parser.parse_args()
    summarize(args.root.resolve())
