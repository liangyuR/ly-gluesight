"""Run selected scenarios against the running Robot/PLC and verify returned results."""
import argparse
import datetime
import json
from pathlib import Path
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from simulator_config import DEFAULT_CONFIG, SCENARIOS, load_config, load_recipe


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
              "scenarios": []}
    output = Path(cfg["runtimeDir"]) / "regression-latest.json"
    active_run = None
    try:
        initial = request(base, "/state")
        expected_recipe = load_recipe(cfg["robot"]["recipe"])
        if not initial.get("canStart"):
            raise RuntimeError("Requires an idle, ready demo with a connected camera bridge: " +
                               initial.get("startBlockedReason", "请重启模拟服务以更新就绪状态"))
        if any(initial["recipe"][key] != expected_recipe[key] for key in ("id", "productCode", "shots")):
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
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"Report: {output}")


if __name__ == "__main__":
    main()
