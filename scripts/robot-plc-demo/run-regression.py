"""Run selected scenarios against the running Robot/PLC and verify returned results."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from simulator_config import DEFAULT_CONFIG, SCENARIOS, load_config, load_recipe, recipe_contract, trigger_plan


def verify_evidence(result, recipe):
    plan, counts = trigger_plan(recipe)
    mismatch = result["scenario"] == "countMismatch"
    expected_counts = dict.fromkeys(counts, 0) if mismatch else counts
    if result.get("deviceTriggers") != expected_counts:
        raise RuntimeError("Accepted device trigger counts differ from the plan")
    acks = result.get("triggerAcks")
    if not isinstance(acks, list) or len(acks) != (0 if mismatch else len(plan)):
        raise RuntimeError("Missing per-shot trigger evidence")
    dlls = {}
    for point, ack in zip(plan, acks):
        identity, evidence = ack.get("identity", {}), ack.get("evidence", {})
        if ack.get("ok") is not True or any(identity.get(key) != value for key, value in point.items()):
            raise RuntimeError("Trigger acknowledgement differs from the shot plan")
        if any(identity.get(key) != value for key, value in {"sn": result["sn"], "recipeId": recipe["id"],
                                                           "productCode": recipe["productCode"], "shotCount": len(plan)}.items()):
            raise RuntimeError("Trigger acknowledgement differs from the current part")
        if any(not result.get(key) or identity.get(key) != result[key] for key in ("cycleId", "recipeRevision", "bundleId")):
            raise RuntimeError("Live verification requires one cycle and immutable release bundle")
        engine, app = evidence.get("engine", {}), evidence.get("app", {})
        if evidence.get("scope") != "demo-app" or engine.get("backend") != "LyFlow" or engine.get("ready") is not True or engine.get("measuring") is not True or not app.get("version"):
            raise RuntimeError("Live verification requires actual demo app and image-engine evidence")
        path = Path(engine.get("path", ""))
        if not path.is_file():
            raise RuntimeError(f"Cannot hash the DLL reported by the demo app: {path}")
        if str(path) not in dlls:
            dlls[str(path)] = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "version": engine.get("version"), "app": app}
    return dlls


def request(base, path, data=None):
    req = Request(base + path, data=None if data is None else json.dumps(data).encode(),
                  headers={"Content-Type": "application/json"})
    try:
        with urlopen(req, timeout=5) as response:
            return json.load(response)
    except HTTPError as exc:
        raise RuntimeError(f"{path}: {exc.code} {exc.read().decode('utf-8')}") from exc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--scenarios", nargs="+", choices=list(SCENARIOS), default=list(SCENARIOS))
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if not 0 < args.timeout <= 600:
        parser.error("--timeout must be in (0, 600]")
    cfg = load_config(args.config)
    base = cfg["consoleUrl"]
    report = {"startedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(), "passed": False,
              "scope": "Live Robot HTTP and Modbus results; image processing is performed by the connected demo app",
              "scenarios": [], "dlls": {}, "physicalValidation": False,
              "unmeasured": ["queue latency", "engine latency", "memory trend", "field image accuracy", "tricam SDK wiring"]}
    output = Path(cfg["runtimeDir"]) / "regression-latest.json"
    active_run = None
    try:
        initial = request(base, "/state")
        expected_recipe = load_recipe(cfg["robot"]["recipe"])
        report.update(fixture=str(Path(cfg["robot"]["recipe"])),
                      fixtureSha256=hashlib.sha256(Path(cfg["robot"]["recipe"]).read_bytes()).hexdigest())
        if not initial.get("canStart"):
            raise RuntimeError("Requires an idle, ready demo with a connected camera bridge: " +
                               initial.get("startBlockedReason", "请重启模拟服务以更新就绪状态"))
        if initial.get("contractVersion") != 2 or recipe_contract(initial["recipe"]) != recipe_contract(expected_recipe):
            raise RuntimeError("Running Robot recipe differs from the configured regression fixture")
        report.update(recipe=initial["recipe"]["id"], previousParts=initial["parts"])
        for scenario in args.scenarios:
            active_run = request(base, "/start", {"scenario": scenario})["runId"]
            deadline = time.monotonic() + args.timeout
            while True:
                state = request(base, "/state")
                if state["runId"] != active_run:
                    raise RuntimeError("A different run started during verification")
                if not state["running"]:
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError(f"{scenario}: timed out")
                time.sleep(0.1)
            if state["phase"] != "complete":
                raise RuntimeError(f"{scenario}: {state['message']}")
            results = [r for r in state["results"] if r.get("runId") == active_run]
            if len(results) != 1:
                raise RuntimeError(f"{scenario}: expected exactly one new result")
            result = results[0]
            expected = SCENARIOS[scenario]
            if (result["resultCode"], result["faultCode"]) != expected or result["resultSn"] != result["sn"]:
                raise RuntimeError(f"{scenario}: unexpected result {result}, expected {expected}")
            if any(state["plc"][key] for key in ("partStart", "partEnd", "resultAck", "armed", "busy", "done")):
                raise RuntimeError(f"{scenario}: handshake did not release")
            report["dlls"].update(verify_evidence(result, expected_recipe))
            report["scenarios"].append(result)
            print(f"PASS {scenario}: SN={result['sn']} result={result['resultCode']} fault={result['faultCode']}", flush=True)
            active_run = None
        report["passed"] = True
    except Exception as exc:
        report["error"] = str(exc)
        if active_run:
            try:
                if request(base, "/state")["runId"] == active_run:
                    request(base, "/stop", {})
            except (OSError, RuntimeError):
                pass
        raise
    finally:
        report["imageProcessingObserved"] = bool(report["dlls"])
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"Report: {output}")


if __name__ == "__main__":
    main()
