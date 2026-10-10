import argparse
import json
import math
import os
import sys
from pathlib import Path


def require(condition, message):
    if not condition:
        raise ValueError(message)


def json_document(data):
    def object_from_pairs(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"Duplicate JSON key: {key}")
            result[key] = value
        return result

    def reject_constant(value):
        raise ValueError(f"Non-finite JSON value: {value}")

    def finite_float(value):
        result = float(value)
        require(math.isfinite(result), f"Non-finite JSON value: {value}")
        return result

    return json.loads(data, object_pairs_hook=object_from_pairs, parse_constant=reject_constant, parse_float=finite_float)


def number(value, label, positive=False):
    require(type(value) in (int, float) and math.isfinite(value) and (value > 0 if positive else value >= 0),
            f"Invalid numeric evidence: {label}")
    return value


def integer(value, label, minimum=0):
    require(type(value) is int and value >= minimum, f"Invalid integer evidence: {label}")
    return value


def close(actual, expected, label):
    require(type(actual) in (int, float) and math.isfinite(actual)
            and math.isclose(actual, expected, rel_tol=1e-9, abs_tol=1e-6), f"Derived evidence differs: {label}")


def validate_distribution(metric, values, label):
    require(values, f"Missing raw metric: {label}")
    values = sorted(number(value, label) for value in values)
    require(integer(metric["count"], label, 1) == len(values) and metric["unit"] == "ms"
            and metric["percentile"] == "nearest rank", f"Incomplete metric: {label}")
    expected = {"p50": values[math.ceil(len(values) * 0.50) - 1], "p95": values[math.ceil(len(values) * 0.95) - 1],
                "min": values[0], "max": values[-1], "mean": sum(values) / len(values)}
    for key, value in expected.items():
        close(metric[key], value, f"{label}.{key}")


def validate_memory(memory, parts):
    samples = memory["samples"]
    expected_parts = [0] + list(range(10, parts + 1, 10))
    if expected_parts[-1] != parts:
        expected_parts.append(parts)
    require([integer(sample["part"], "memory.part") for sample in samples] == expected_parts,
            "Memory sampling does not span the steady run at every 10 parts")
    times = [number(sample["elapsedMs"], "memory.elapsedMs") for sample in samples]
    require(all(a < b for a, b in zip(times, times[1:])), "Memory sampling times are not increasing")
    for key, metric_name in (("workingSetBytes", "workingSet"), ("privateBytes", "private")):
        values = [sample[key] for sample in samples]
        metric = memory[metric_name]
        if all(value is None for value in values):
            require(metric is None, f"Unknown memory metric was invented: {metric_name}")
            continue
        require(all(type(value) is int and value > 0 for value in values), f"Incomplete process memory evidence: {key}")
        require(isinstance(metric, dict), f"Missing memory trend: {metric_name}")
        mean_x, mean_y = sum(expected_parts) / len(samples), sum(values) / len(samples)
        denominator = sum((x - mean_x) ** 2 for x in expected_parts)
        slope = sum((x - mean_x) * (y - mean_y) for x, y in zip(expected_parts, values)) / denominator
        expected = {"startBytes": values[0], "endBytes": values[-1], "deltaBytes": values[-1] - values[0],
                    "minBytes": min(values), "maxBytes": max(values), "linearSlopeBytesPerPart": slope}
        for field, value in expected.items():
            close(metric[field], value, f"memory.{metric_name}.{field}")


def validate_unknown_metrics(case):
    for key in ("productionQueueWaitMs", "productionPartEndToJudgeMs"):
        require(case[key] is None, f"Serial direct harness cannot claim production timing: {key}")


def validate_jsonl(artifact, records):
    persisted = [json_document(line) for line in Path(artifact["path"]).read_text(encoding="utf-8").splitlines()]
    require(persisted == records, "Persisted per-part evidence differs from report snapshot")


def validate_fixture_pixels(record, size, background):
    header = f"P5\n{size[0]} {size[1]}\n255\n".encode("ascii")
    path = Path(record["image"]["path"])
    data = path.read_bytes()
    require(data.startswith(header) and len(data) == len(header) + size[0] * size[1],
            f"Fixture is not full-resolution raw PGM: {path}")
    pixels = data[len(header):]
    x, y = record["probePx"]
    require(type(x) is int and type(y) is int and 0 <= x < size[0] and 0 <= y < size[1], "Invalid fixture probe coordinate")
    probe = pixels[y * size[0] + x]
    require(probe == record["probePixel"], f"Fixture pixel probe differs: {path}")
    require(min(pixels) > 120 if background else probe < 70, f"Fixture selected/background pixels differ: {path}")


def verify_artifacts(value, cache=None):
    if cache is None:
        cache = {}
    if isinstance(value, dict):
        if {"path", "bytes"} <= value.keys():
            path = Path(value["path"])
            require(path.is_absolute(), f"Artifact path must be absolute: {path}")
            integer(value["bytes"], f"artifact.bytes {path}")
            key = os.path.normcase(str(path.absolute()))
            size = path.stat().st_size
            require(size == value["bytes"], f"Artifact size changed: {path}")
            cache[key] = size
        for child in value.values():
            verify_artifacts(child, cache)
    elif isinstance(value, list):
        for child in value:
            verify_artifacts(child, cache)
    return cache


def check_judgement(record, verdict, plc, fault, errors):
    result = record["judgement"]
    integer(result["plcCode"], "plcCode")
    integer(result["faultCode"], "faultCode")
    require((result["verdict"], result["plcCode"], result["faultCode"]) == (verdict, plc, fault),
            f"Unexpected judgement: {result}")
    require(integer(record["measurementErrors"], "measurementErrors") == errors, "Unexpected native-error count")


def validate_calls(record, case, shots):
    calls = record["calls"]
    require(len(calls) == 4, "Missing native measurements")
    for k, call in enumerate(calls):
        require(integer(call["k"], "call.k") == k and call["shotId"] == shots[k]["shotId"]
                and call["camera"] == "cam1" and call["view"] == case["shotViews"][k], "Measurement identity differs")
        require(call["size"] == [1280, 1024], "Measurement resolution differs")
        wall = number(call["preparedMeasureMs"], "preparedMeasureMs", positive=True)
        if call["error"] is None:
            engine = number(call["engineRunIoParseMs"], "engineRunIoParseMs")
            require(engine <= wall + 1.0, "Native run duration exceeds the containing Prepared call")
            require(integer(call["stations"], "stations") == 101, "Measurement station count differs")
            coverage = number(call["coverage"], "coverage")
            require(coverage <= 1 and integer(call["gapStations"], "gapStations") <= 101, "Invalid native coverage/gap count")
            lo, hi = (number(call[key], key, positive=True) for key in ("widthMinMm", "widthMaxMm"))
            require(lo <= hi, "Invalid native width bounds")
        else:
            require(isinstance(call["error"], str) and call["error"], "Native error must be explicit")
            require(call["engineRunIoParseMs"] is None, "Failed native call has no completed run timing")
    require(sum(call["error"] is not None for call in calls) == record["measurementErrors"], "Native error details differ")
    total = number(record["serialPartMeasureAndJudgeMs"], "serialPartMeasureAndJudgeMs", positive=True)
    tail = number(record["lastSelectedFrameToJudgeMs"], "lastSelectedFrameToJudgeMs", positive=True)
    require(total + 1e-6 >= sum(call["preparedMeasureMs"] for call in calls), "Serial part time omits native calls")
    require(calls[-1]["preparedMeasureMs"] <= tail + 1e-6 and tail <= total + 1e-6, "Serial last-selected-image tail is inconsistent")


def validate_case(case):
    require(case["passed"] is True, "Case did not pass")
    count = integer(case["viewCount"], "viewCount", 1)
    require(count in (1, 3), "Only single-view or three-view cases are valid")
    require(case["physicalDevices"] == 1, "Each regression case must use one physical-device identity")
    require(case["shotViews"] == ([1, 1, 1, 1] if count == 1 else [1, 2, 3, 1]), "Incorrect selected-view plan")
    require(case["imageSize"] == [1280, 1024], "Regression must use actual 1280x1024 image size")
    require(case["shotsPerPart"] == case["deviceFramesPerPart"] == case["selectedMeasurementsPerPart"] == 4,
            "A three-view device still has four device frames and four selected measurements per part")
    require(case["viewImagesPerPart"] == 4 * count, "Incorrect source-image count")
    require(integer(case["steadyParts"], "steadyParts", 100) >= 100, "At least 100 steady parts are required per case")
    require(case["steadyMeasurements"] == case["steadyParts"] * 4, "Incorrect stable-measurement count")
    require(len(case["steadyRecords"]) == case["steadyParts"], "Missing per-part evidence")
    validate_jsonl(case["steadyEvidence"], case["steadyRecords"])
    manifest_path = Path(case["releaseManifest"]["path"])
    manifest = json_document(manifest_path.read_text(encoding="utf-8"))
    require(manifest["bundleId"] == case["bundleId"], "Release bundle ID differs")
    require(manifest["recipeRevision"] == case["recipeRevision"], "Frozen recipe identity differs")
    require(len(manifest["shots"]) == 4 and [s["view"] for s in manifest["shots"]] == case["shotViews"], "Frozen shot plan differs")
    require([shot["k"] for shot in manifest["shots"]] == list(range(4)) and all(not shot["skip"] for shot in manifest["shots"]), "Frozen measurements must cover four ordered shots")
    require(all(s["camera"] == "cam1" and s["size"] == [1280, 1024] for s in manifest["shots"]), "Frozen camera or image size differs")
    resources = {Path(a["path"]).resolve(): a for a in case["releaseResources"]}
    require(len(resources) == len(case["releaseResources"]), "Duplicate reported release resource")
    for frozen in manifest["files"]:
        relative = Path(frozen["path"])
        require(not relative.is_absolute() and ".." not in relative.parts, "Frozen resource path escapes its bundle")
        path = (manifest_path.parent / relative).resolve()
        require(path.is_relative_to(manifest_path.parent.resolve()), "Frozen resource path escapes its bundle")
        require(path in resources, f"Unreported frozen resource: {path}")
        require(resources[path]["bytes"] == frozen["bytes"], "Frozen resource size differs")
    require(len(resources) == len(manifest["files"]), "Unexpected reported release resource")
    for ordinal, part in enumerate(case["steadyRecords"], 1):
        require(integer(part["part"], "part", 1) == ordinal, "Steady parts are missing, repeated or out of order")
        check_judgement(part, "OK", 1, 0, 0)
        validate_calls(part, case, manifest["shots"])
        for call in part["calls"]:
            require(call["error"] is None and call["coverage"] >= 0.95, "Normal pixels did not produce valid native measurements")
    probes = case["scenarioProbes"]
    check_judgement(probes["partialGap"], "NG_GAP", 13, 0, 0)
    validate_calls(probes["partialGap"], case, manifest["shots"])
    require(probes["partialGap"]["calls"][1]["gapStations"] > 6, "Partial gap was not measured from pixels")
    check_judgement(probes["wholeEmpty"], "ERR_INSPECT", 90, 99, 4)
    validate_calls(probes["wholeEmpty"], case, manifest["shots"])
    require(all("检测区内没找到胶" in c["error"] for c in probes["wholeEmpty"]["calls"]), "Whole-empty result is not an explicit native no-bead error")
    validate_unknown_metrics(case)
    for key in ("preparedMeasureMs", "engineRunIoParseMs"):
        validate_distribution(case[key], [call[key] for part in case["steadyRecords"] for call in part["calls"]], key)
    for key in ("serialPartMeasureAndJudgeMs", "lastSelectedFrameToJudgeMs"):
        validate_distribution(case[key], [part[key] for part in case["steadyRecords"]], key)
    for key in ("fixtureRenderMs", "publishMs", "preparedLoadMs", "warmupMs", "steadyTotalMs"):
        number(case[key], key, positive=True)
    require(case["steadyTotalMs"] + 1e-6 >= sum(part["serialPartMeasureAndJudgeMs"] for part in case["steadyRecords"]),
            "Steady-loop duration omits measured parts")
    require(case["warmupMeasurements"] == 4 and case["scenarioProbeMeasurements"] == 8, "Missing warmup/scenario measurements")
    validate_memory(case["memory"], case["steadyParts"])
    fixture = json_document(Path(case["fixtures"]["path"]).read_text(encoding="utf-8"))
    require(fixture == case["fixtureManifest"], "Persisted fixture manifest differs from report snapshot")
    require(fixture["imageSize"] == [1280, 1024], "Fixture manifest resolution differs")
    require([scenario["name"] for scenario in fixture["scenarios"]] == ["normal", "partial_gap", "whole_empty"], "Missing or repeated fixture scenario")
    for scenario in fixture["scenarios"]:
        require(len(scenario["shots"]) == 4, "Missing fixture shot")
        for k, shot in enumerate(scenario["shots"]):
            require(shot["k"] == k and shot["shotId"] == manifest["shots"][k]["shotId"] and shot["camera"] == "cam1", "Fixture shot identity differs")
            require(shot["selectedView"] == case["shotViews"][k] and len(shot["images"]) == count, "Fixture view binding differs")
            require([image["view"] for image in shot["images"]] == list(range(1, count + 1)), "Fixture views are missing or repeated")
            require(sum(image["selected"] for image in shot["images"]) == 1, "Each device frame must select exactly one view")
            for image in shot["images"]:
                require(type(image["selected"]) is bool and image["selected"] == (image["view"] == shot["selectedView"]), "Fixture selected-view flag differs")
                validate_fixture_pixels(image, case["imageSize"], scenario["name"] == "whole_empty" or not image["selected"])


def enrich(report_path):
    raw = report_path.read_bytes()
    report = json_document(raw)
    require(report["schemaVersion"] == 1 and report["passed"] is True, "Unsupported or failed native report")
    require([case["viewCount"] for case in report["cases"]] == [1, 3], "Both single-view and three-view regression are required in warmup order")
    number(report["engine"]["loadAndSelfCheckMs"], "loadAndSelfCheckMs", positive=True)
    require(report["software"]["sourceFiles"] and report["software"]["testExecutable"]["bytes"] > 0, "Missing software artifact evidence")
    for ordinal, case in enumerate(report["cases"]):
        require(case["enginePreviouslyUsedInProcess"] is bool(ordinal), "Incorrect cold/warm engine-use claim")
        require(case["warmupKind"] == ("already_used_engine" if ordinal else "first_graph_use_for_loaded_engine"), "Incorrect warmup scope")
        validate_case(case)
        manifest = json_document(Path(case["releaseManifest"]["path"]).read_text(encoding="utf-8"))
        require(manifest["versions"]["engine"] == report["engine"]["version"], "Frozen engine identity differs")
        require(manifest["versions"]["graph"] == report["software"]["graphVersion"], "Frozen graph version differs")
        if report["os"] == "windows":
            require(all(type(s["privateBytes"]) is int and s["privateBytes"] > 0
                        and s["source"] == "GetProcessMemoryInfo/PROCESS_MEMORY_COUNTERS_EX" for s in case["memory"]["samples"]), "Missing Windows process memory evidence")
    cache = verify_artifacts(report)
    verifier_path = Path(__file__).resolve(strict=True)
    report["artifactVerification"] = {"passed": True, "uniqueFiles": len(cache),
                                      "verifier": {"path": str(verifier_path), "bytes": verifier_path.stat().st_size},
                                      "scope": "Reads actual full-resolution fixture pixels and business fields; checks paths, dimensions and counts; recomputes latency and memory. No content fingerprints or content matching. Does not authenticate execution of an untrusted report or prove production scheduling."}
    return report


def summary(report):
    print(f'GlueSight {report["software"]["version"]}; core {report["engine"]["version"]}; git {report["software"]["gitCommit"] or "unknown"}')
    print(f'DLL load/self-check {report["engine"]["loadAndSelfCheckMs"]:.3f} ms')
    for case in report["cases"]:
        wall, engine, part, tail = (case[k] for k in ("preparedMeasureMs", "engineRunIoParseMs", "serialPartMeasureAndJudgeMs", "lastSelectedFrameToJudgeMs"))
        print(f'{case["mode"]}: {case["steadyParts"]} parts / {case["steadyMeasurements"]} measures; warmup {case["warmupMs"]:.3f} ms ({case["warmupKind"]})')
        print(f'  Prepared P50/P95/max {wall["p50"]:.3f}/{wall["p95"]:.3f}/{wall["max"]:.3f} ms; run/I/O/parse {engine["p50"]:.3f}/{engine["p95"]:.3f}/{engine["max"]:.3f} ms')
        print(f'  Last selected preloaded frame to judge P50/P95/max {tail["p50"]:.3f}/{tail["p95"]:.3f}/{tail["max"]:.3f} ms; bundle {case["bundleId"]}')
        print(f'  Serial four-measurement part P50/P95/max {part["p50"]:.3f}/{part["p95"]:.3f}/{part["max"]:.3f} ms; fixture manifest {case["fixtures"]["path"]}')
        private = case["memory"]["private"]
        if private:
            print(f'  Private memory delta {private["deltaBytes"]} bytes; slope {private["linearSlopeBytesPerPart"]:.1f} bytes/part')
        else:
            print("  Process memory trend: unavailable on this platform")
    print("Scope: serial direct Prepared calls. Production queue wait and actual partEnd-to-judge tail are unknown; PLC, camera transport, physical SDK synchronization and Recorder latency are not measured.")
    print("Cold scope: first graph use of this benchmark's loaded Engine. Process isolation and OS/DLL file caches are not controlled by the harness.")


def self_test():
    import tempfile
    import unittest

    class ReportTests(unittest.TestCase):
        def test_missing_artifacts_are_rejected_without_content_matching(self):
            with tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / "image.pgm"
                path.write_bytes(b"pixels")
                artifact = {"path": str(path), "bytes": 6}
                self.assertEqual(len(verify_artifacts(artifact)), 1)
                path.write_bytes(b"edited")
                self.assertEqual(len(verify_artifacts(artifact)), 1)
                path.unlink()
                with self.assertRaises(FileNotFoundError):
                    verify_artifacts(artifact)

        def test_size_and_relative_paths_are_rejected(self):
            with self.assertRaisesRegex(ValueError, "absolute"):
                verify_artifacts({"path": "relative.pgm", "bytes": 0})
            with tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / "image.pgm"
                path.write_bytes(b"pixels")
                with self.assertRaisesRegex(ValueError, "size changed"):
                    verify_artifacts({"path": str(path), "bytes": 1})

        def test_empty_pixels_cannot_be_relabelled_ok(self):
            with self.assertRaisesRegex(ValueError, "Unexpected judgement"):
                check_judgement({"judgement": {"verdict": "OK", "plcCode": 1, "faultCode": 0}, "measurementErrors": 4}, "ERR_INSPECT", 90, 99, 4)

        def test_json_rejects_duplicate_keys_and_nonfinite_values(self):
            with self.assertRaisesRegex(ValueError, "Duplicate JSON key"):
                json_document('{"passed":false,"passed":true}')
            for value in ("NaN", "Infinity", "-Infinity", "1e999"):
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, "Non-finite JSON"):
                    json_document('{"duration":' + value + '}')

        def test_ordered_but_fabricated_latency_summary_is_rejected(self):
            metric = {"count": 4, "p50": 2.0, "p95": 4.0, "min": 1.0, "max": 4.0,
                      "mean": 2.5, "unit": "ms", "percentile": "nearest rank"}
            validate_distribution(metric, [4.0, 1.0, 3.0, 2.0], "latency")
            for key, value in (("p50", 3.0), ("max", 5.0), ("mean", 3.0)):
                with self.subTest(key=key), self.assertRaisesRegex(ValueError, "Derived evidence differs"):
                    validate_distribution(dict(metric, **{key: value}), [1.0, 2.0, 3.0, 4.0], "latency")
            for invalid in (False, float("nan"), float("inf"), -1.0):
                with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, "Invalid numeric"):
                    validate_distribution(metric, [invalid], "latency")
            with self.assertRaisesRegex(ValueError, "Missing raw metric"):
                validate_distribution(metric, [], "latency")

        def test_memory_trend_is_recomputed_and_sampling_gaps_are_rejected(self):
            samples = [{"part": part, "elapsedMs": part + 0.1, "workingSetBytes": 1024 + 2 * part,
                        "privateBytes": 2048 + 3 * part} for part in range(0, 101, 10)]
            memory = {"samples": samples,
                      "workingSet": {"startBytes": 1024, "endBytes": 1224, "deltaBytes": 200,
                                     "minBytes": 1024, "maxBytes": 1224, "linearSlopeBytesPerPart": 2.0},
                      "private": {"startBytes": 2048, "endBytes": 2348, "deltaBytes": 300,
                                  "minBytes": 2048, "maxBytes": 2348, "linearSlopeBytesPerPart": 3.0}}
            validate_memory(memory, 100)
            with self.assertRaisesRegex(ValueError, "Derived evidence differs"):
                validate_memory(dict(memory, private=dict(memory["private"], linearSlopeBytesPerPart=0)), 100)
            with self.assertRaisesRegex(ValueError, "every 10 parts"):
                validate_memory(dict(memory, samples=samples[:4] + samples[5:]), 100)
            unavailable = {"samples": [dict(sample, workingSetBytes=None, privateBytes=None) for sample in samples],
                           "workingSet": None, "private": None}
            validate_memory(unavailable, 100)
            with self.assertRaisesRegex(ValueError, "invented"):
                validate_memory(dict(unavailable, private=memory["private"]), 100)

        def test_actual_production_tail_cannot_be_invented_from_serial_calls(self):
            unknown = {"productionQueueWaitMs": None, "productionPartEndToJudgeMs": None}
            validate_unknown_metrics(unknown)
            for key in unknown:
                with self.subTest(key=key), self.assertRaisesRegex(ValueError, "cannot claim production timing"):
                    validate_unknown_metrics(dict(unknown, **{key: 0}))

        def test_streamed_per_part_evidence_must_match_report(self):
            with tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / "steady-records.jsonl"
                records = [{"part": 1, "duration": 2.0}, {"part": 2, "duration": 3.0}]
                path.write_text("\n".join(json.dumps(record) for record in records) + "\n", encoding="utf-8")
                validate_jsonl({"path": str(path)}, records)
                path.write_text(json.dumps(records[0]) + "\n", encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "per-part evidence differs"):
                    validate_jsonl({"path": str(path)}, records)

        def test_fixture_resolution_and_probe_are_read_from_real_pgm(self):
            with tempfile.TemporaryDirectory() as folder:
                path = Path(folder) / "full-size.pgm"
                pixels = bytearray([150]) * (1280 * 1024)
                pixels[432 * 1280 + 840] = 30
                header = b"P5\n1280 1024\n255\n"
                path.write_bytes(header + pixels)
                record = {"image": {"path": str(path)}, "probePx": [840, 432], "probePixel": 30}
                validate_fixture_pixels(record, [1280, 1024], background=False)
                pixels[432 * 1280 + 840] = 151
                path.write_bytes(header + pixels)
                with self.assertRaisesRegex(ValueError, "pixel probe differs"):
                    validate_fixture_pixels(record, [1280, 1024], background=False)
                path.write_bytes(b"P5\n1 1\n255\n\x1e")
                with self.assertRaisesRegex(ValueError, "not full-resolution raw PGM"):
                    validate_fixture_pixels(record, [1280, 1024], background=False)

        def test_native_call_timings_and_last_selected_tail_must_be_consistent(self):
            import copy

            shots = [{"shotId": f"P{k + 1}"} for k in range(4)]
            case = {"shotViews": [1, 2, 3, 1]}
            calls = [{"k": k, "shotId": shots[k]["shotId"], "camera": "cam1", "view": case["shotViews"][k],
                      "size": [1280, 1024], "preparedMeasureMs": 2.0, "engineRunIoParseMs": 1,
                      "stations": 101, "coverage": 1.0, "gapStations": 0, "widthMinMm": 4.0,
                      "widthMaxMm": 4.0, "error": None} for k in range(4)]
            record = {"calls": calls, "measurementErrors": 0, "serialPartMeasureAndJudgeMs": 9.0,
                      "lastSelectedFrameToJudgeMs": 2.5}
            validate_calls(record, case, shots)
            for field, value, message in (("serialPartMeasureAndJudgeMs", 7.0, "omits native calls"),
                                          ("lastSelectedFrameToJudgeMs", 1.0, "tail is inconsistent")):
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, message):
                    validate_calls(dict(record, **{field: value}), case, shots)
            zero = copy.deepcopy(record)
            zero["calls"][0]["preparedMeasureMs"] = 0
            with self.assertRaisesRegex(ValueError, "Invalid numeric"):
                validate_calls(zero, case, shots)

    result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(ReportTests))
    return 0 if result.wasSuccessful() else 1


def main():
    parser = argparse.ArgumentParser(description="Verify real P0 DLL regression business fields, image dimensions, measurements and derived metrics.")
    parser.add_argument("report", nargs="?", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if args.report is None:
        parser.error("report.json is required unless --self-test is used")
    report = enrich(args.report)
    output = args.output or args.report.with_name("report.evidence.json")
    require(output.resolve() != args.report.resolve(), "Preserve the raw report; choose a separate output path")
    require(not output.exists(), f"Evidence output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8", newline="\n") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2, allow_nan=False)
        handle.write("\n")
    summary(report)
    print(f"Verified evidence: {output.resolve()}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Evidence verification failed: {error}", file=sys.stderr)
        sys.exit(1)
