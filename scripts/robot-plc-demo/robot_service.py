"""Point-to-point Robot model with virtual camera pulses and real Modbus handshakes."""
import argparse
import datetime
import hashlib
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import threading
import time
import uuid

from modbus_wire import Client, snapshot
from simulator_config import DEFAULT_CONFIG, SCENARIOS, integer, load_config, load_recipe, trigger_plan, validate_recipe


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


class Conflict(RuntimeError):
    pass


def start_blocked_reason(state):
    """One readiness rule for HTTP clients and the browser console."""
    if state["running"]:
        return ("已收到停止请求，正在完成本件，请等待握手释放。" if state["stopping"] else
                "Robot 正在运行，请等待本件完成，或点击“本件后停止”。")
    plc = state.get("plc")
    if plc is None:
        return "PLC 服务未连接，请检查模拟服务，连接恢复后重试。"
    if not state["bridgeOnline"]:
        return "相机触发桥离线，请确认专用 GlueSight 窗口与触发桥正在运行。"
    if not plc["visionReady"]:
        return "视觉尚未就绪，请在 GlueSight 检查相机、算法引擎、已发布配方及 PLC 连接。"
    if plc["done"]:
        return "上一件结果尚未释放，请等待结果确认；流程已停止时可执行故障复位。"
    if plc["armed"] or plc["busy"]:
        return "检测流程尚未结束，请等待本件完成；流程异常时可执行故障复位。"
    if any(plc[key] for key in ("partStart", "partEnd", "resultAck", "faultReset")):
        return "上一件握手信号尚未释放，请故障复位后重试。"
    return None


class Robot:
    def __init__(self, config, recipe, output):
        self.config, self.recipe = config, validate_recipe(recipe)
        self.plan, self.planned_triggers = trigger_plan(recipe)
        self.recipe_sha256 = hashlib.sha256(json.dumps(recipe, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()
        self.output = Path(output)
        self.output.mkdir(parents=True, exist_ok=True)
        self.lock = threading.RLock()
        self.stop = threading.Event()
        self.pending = None
        self.bridge_seen = float("-inf")
        self.thread = None
        self.session = uuid.uuid4().hex
        self.last_sn = 0
        self.state = {"running": False, "stopping": False, "phase": "idle", "scenario": "normal", "sn": None,
                      "k": None, "shotId": None, "poseId": None, "camera": None, "view": None,
                      "deviceTriggers": {}, "parts": 0, "message": "Robot 服务已就绪",
                      "runId": None, "cycleId": None, "recipeHash": None, "bundleHash": None,
                      "results": [], "events": []}
        results_file = self.output / "robot-results.jsonl"
        if results_file.exists():
            previous = []
            lines = results_file.read_text(encoding="utf-8").splitlines()
            for index, line in enumerate(lines):
                if not line.strip():
                    continue
                try:
                    item = json.loads(line)
                    integer(item["sn"], 1, 0xFFFFFFFF, "saved sn")
                    previous.append(item)
                except (ValueError, KeyError):
                    # A crash can leave one incomplete final append. Keep earlier records.
                    if index != len(lines) - 1:
                        raise ValueError(f"Corrupt Robot result at line {index + 1}")
                    self.event("warning", "上次结果文件末行不完整，已保留备份并恢复完整记录")
                    backup = results_file.with_name(f"robot-results.interrupted-{self.session}.jsonl")
                    backup.write_bytes(results_file.read_bytes())
                    results_file.write_text("".join(json.dumps(r) + "\n" for r in previous), encoding="utf-8")
            self.last_sn = max((r["sn"] for r in previous), default=0)
            self.state.update(parts=len(previous), results=previous[-100:],
                              message=f"Robot 服务已就绪，已恢复 {len(previous)} 件结果")

    def client(self):
        return Client(self.config["plc"]["port"], self.config["plc"]["unitId"])

    def event(self, phase, message, **data):
        item = {"ts": utc_now(), "phase": phase, "message": message, **data}
        with self.lock:
            self.state.update(phase=phase, message=message)
            self.state["events"] = (self.state["events"] + [item])[-50:]
            with (self.output / "robot-events.jsonl").open("a", encoding="utf-8") as file:
                file.write(json.dumps(item, ensure_ascii=False) + "\n")

    def online(self):
        return time.monotonic() - self.bridge_seen < 3

    def snapshot(self):
        with self.lock:
            response = json.loads(json.dumps(self.state))
            response.update(contractVersion=2, bridgeOnline=self.online(), recipe=self.recipe,
                            plannedTriggers=self.planned_triggers, recipeSha256=self.recipe_sha256,
                            plcEndpoint=f'127.0.0.1:{self.config["plc"]["port"]}',
                            unitId=self.config["plc"]["unitId"])
        try:
            with self.client() as client:
                response["plc"] = snapshot(client)
        except (OSError, EOFError, ValueError, RuntimeError) as exc:
            response["plcError"] = str(exc)
        response["startBlockedReason"] = start_blocked_reason(response)
        response["canStart"] = response["startBlockedReason"] is None
        response["canReset"] = not response["running"] and "plc" in response
        return response

    def wait_for(self, predicate, stage, message):
        deadline = time.monotonic() + self.config["robot"]["timeouts"][stage]
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(0.02)
        raise TimeoutError(message)

    def poll_trigger(self):
        with self.lock:
            self.bridge_seen = time.monotonic()
            return ({key: value for key, value in self.pending.items() if key not in ("done", "reply")}
                    if self.pending and not self.pending["done"].is_set() else None)

    def acknowledge_trigger(self, data):
        if type(data.get("ok")) is not bool:
            raise ValueError("Trigger acknowledgement requires boolean ok")
        with self.lock:
            if not self.pending or self.pending["id"] != data.get("id"):
                raise Conflict("No matching trigger")
            if self.pending["done"].is_set():
                if self.pending["reply"] != data:
                    raise Conflict("Trigger has already been acknowledged")
                return
            if data["ok"]:
                identity = data.get("identity")
                if not isinstance(identity, dict):
                    raise ValueError("Trigger acknowledgement requires the armed cycle identity")
                for key in ("sn", "k", "recipeId", "productCode", "shotCount", "shotId", "poseId", "camera", "view", "ordinal"):
                    if type(identity.get(key)) is not type(self.pending[key]) or identity[key] != self.pending[key]:
                        raise ValueError(f"Trigger acknowledgement {key} differs from the ticket")
                for key in ("cycleId", "recipeHash"):
                    if not isinstance(identity.get(key), str) or not identity[key]:
                        raise ValueError(f"Trigger acknowledgement requires {key}")
                if identity.get("bundleHash") is not None and not isinstance(identity["bundleHash"], str):
                    raise ValueError("Trigger bundleHash must be a string or null")
                if self.pending["k"] > 0 and any(identity.get(key) != self.state[key] for key in ("cycleId", "recipeHash", "bundleHash")):
                    raise Conflict("Cycle or frozen resources changed between triggers")
                self.state.update({key: identity.get(key) for key in ("cycleId", "recipeHash", "bundleHash")})
            self.pending["reply"] = data
            self.pending["done"].set()

    def trigger(self, sn, k, scenario):
        ticket = {"id": f"{self.session}:{sn}:{k}", "sn": sn, "k": k, "scenario": scenario,
                  "recipeId": self.recipe["id"], "productCode": self.recipe["productCode"],
                  "shotCount": len(self.plan), **self.plan[k],
                  "reply": None, "done": threading.Event()}
        with self.lock:
            self.pending = ticket
        try:
            if not ticket["done"].wait(self.config["robot"]["timeouts"]["trigger"]):
                raise TimeoutError("虚拟相机触发桥未响应")
            if not ticket["reply"]["ok"]:
                raise RuntimeError(ticket["reply"].get("error", "相机未接受触发"))
            return ticket["reply"]
        finally:
            with self.lock:
                if self.pending is ticket:
                    self.pending = None

    def start(self, data):
        scenario = data.get("scenario", "normal")
        if scenario not in SCENARIOS:
            raise ValueError("Unknown scenario")
        count = integer(data.get("count", 1), 1, 100, "count")
        continuous = data.get("continuous", False)
        if type(continuous) is not bool:
            raise ValueError("continuous must be boolean")
        with self.lock:
            if self.state["running"]:
                raise Conflict(start_blocked_reason(self.state))
            # Check the current PLC values while holding the same lock used by
            # start/reset/stop. A stale browser snapshot must not launch a part.
            reason = self.snapshot()["startBlockedReason"]
            if reason:
                raise Conflict(reason)
            self.stop.clear()
            run_id = uuid.uuid4().hex
            self.state.update(running=True, stopping=False, runId=run_id)
            self.thread = threading.Thread(target=self.run, args=(scenario, continuous, count, run_id), daemon=True)
            self.thread.start()
            return {"started": True, "runId": run_id}

    def request_stop(self):
        with self.lock:
            if not self.state["running"]:
                return {"stopping": False, "message": "Robot 已停止，无需重复操作"}
            if not self.state["stopping"]:
                self.state["stopping"] = True
                self.stop.set()
                self.event("stopping", "已收到停止请求，将完成本件并释放握手后停止",
                           runId=self.state["runId"])
            return {"stopping": True, "message": "已收到停止请求，将完成本件并释放握手后停止"}

    def reset(self):
        # Serialize reset and start so reset cannot pulse during a newly started part.
        with self.lock:
            if self.state["running"]:
                raise Conflict("先等本件停止")
            with self.client() as client:
                for address in (10, 11, 12):
                    client.coil(address, False)
                client.coil(13, True)
                time.sleep(0.2)
                client.coil(13, False)
            self.event("reset_requested", "故障复位请求已发送，请确认视觉与握手恢复就绪后再运行")

    def run(self, scenario, continuous, count, run_id):
        client = None
        completed = 0
        try:
            client = self.client()
            timing = self.config["robot"]
            while not self.stop.is_set() and (continuous or completed < count):
                self.last_sn = (max(self.last_sn + 1, int(time.time() * 10) % 0xFFFFFFFF) % 0x100000000) or 1
                sn, n = self.last_sn, len(self.recipe["shots"])
                with self.lock:
                    self.state.update(sn=sn, k=None, shotId=None, poseId=None, camera=None, view=None,
                                      deviceTriggers=dict.fromkeys(self.planned_triggers, 0), scenario=scenario,
                                      cycleId=None, recipeHash=None, bundleHash=None)
                started_at = time.monotonic()
                trigger_ack_ms = []
                trigger_acks = []
                for address in (10, 11, 12, 13):
                    client.coil(address, False)
                self.event("waiting_ready", "等待视觉就绪", sn=sn)
                self.wait_for(lambda: self.stop.is_set() or client.coils(20, 4) == [True, False, False, False],
                              "ready", "视觉未就绪或上一件未释放")
                if self.stop.is_set():
                    break
                shot_count = n - 1 if scenario == "countMismatch" else n
                client.write_registers(100, [sn >> 16, sn & 65535, self.recipe["productCode"], shot_count, 0, 0])
                client.coil(10, True)
                self.event("starting", "工件开始，等待布防", sn=sn, productCode=self.recipe["productCode"], shots=shot_count)
                def armed_or_done():
                    flags = client.coils(21, 3)
                    return flags if flags[0] or flags[2] else None
                flags = self.wait_for(armed_or_done, "armed", "未收到 armed 或 done")
                if flags[0] and not flags[2]:
                    for point in self.plan:
                        k = point["k"]
                        self.event("moving", f"移动到 {point['shotId']} · Pose {point['poseId']}", sn=sn, **point)
                        time.sleep(timing["motionSeconds"])
                        with self.lock:
                            self.state.update(**point)
                        triggered_at = time.monotonic()
                        trigger_acks.append(self.trigger(sn, k, scenario))
                        trigger_ack_ms.append(round((time.monotonic() - triggered_at) * 1000, 2))
                        with self.lock:
                            self.state["deviceTriggers"][point["camera"]] += 1
                        self.event("triggered", f"{point['shotId']} · {point['camera']} 第 {point['ordinal']} 次触发已接受 · 视角 {point['view']}", sn=sn, **point)
                    time.sleep(timing["settleSeconds"])
                    client.coil(11, True)
                    self.event("waiting_result", "运动结束，等待检测结果", sn=sn)
                result_wait_at = time.monotonic()
                self.wait_for(lambda: client.coils(23, 1)[0], "result", "未收到 done")
                result_code, fault_code, high, low = client.registers(110, 4)
                result_sn = (high << 16) | low
                if result_sn != sn:
                    raise RuntimeError(f"结果 SN 不匹配：{result_sn} != {sn}")
                result = {"sn": sn, "scenario": scenario, "resultCode": result_code, "faultCode": fault_code,
                          "resultSn": result_sn, "ts": utc_now(), "runId": run_id,
                          "recipeSha256": self.recipe_sha256, "deviceTriggers": dict(self.state["deviceTriggers"]),
                          "cycleId": self.state["cycleId"], "recipeHash": self.state["recipeHash"],
                          "bundleHash": self.state["bundleHash"], "triggerAcks": trigger_acks,
                          "triggerAckMs": trigger_ack_ms, "resultWaitMs": round((time.monotonic() - result_wait_at) * 1000, 2),
                          "cycleMs": round((time.monotonic() - started_at) * 1000, 2)}
                self.event("acknowledging", "结果已收到，发送确认", **result)
                client.coil(12, True)
                self.wait_for(lambda: not client.coils(23, 1)[0], "release", "结果确认后 done 未复位")
                for address in (10, 11, 12):
                    client.coil(address, False)
                self.wait_for(lambda: not any(client.coils(21, 3)), "release", "检测信号未释放")
                # Outputs are already low when resultAck is consumed. Hold inputs low
                # long enough for the vision poller to observe release before a new run.
                time.sleep(timing["releaseSeconds"])
                with self.lock:
                    with (self.output / "robot-results.jsonl").open("a", encoding="utf-8") as file:
                        file.write(json.dumps(result) + "\n")
                    self.state["results"] = (self.state["results"] + [result])[-100:]
                    self.state["parts"] += 1
                self.event("complete", f"SN {sn} 已完成，结果码 {result_code}", **result)
                completed += 1
                if continuous or completed < count:
                    self.stop.wait(timing["intervalSeconds"])
        except Exception as exc:
            self.event("error", str(exc), runId=run_id)
        finally:
            if client:
                try:
                    for address in (10, 11, 12):
                        client.coil(address, False)
                except (OSError, EOFError, ValueError, RuntimeError):
                    pass
                client.close()
            with self.lock:
                if self.state["stopping"] and self.state["phase"] != "error":
                    self.event("stopped", f"已停止，本次完成 {completed} 件", runId=run_id)
                self.state.update(running=False, stopping=False)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def reply(self, obj, status=200):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        static_files = {"/": ("console.html", "text/html"),
                        "/console.mjs": ("console.mjs", "text/javascript")}
        if self.path in static_files:
            name, content_type = static_files[self.path]
            body = Path(__file__).with_name(name).read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", content_type + "; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)
        elif self.path == "/state":
            self.reply(self.server.robot.snapshot())
        elif self.path == "/trigger":
            self.reply(self.server.robot.poll_trigger())
        else:
            self.reply({"error": "Not found"}, 404)

    def do_POST(self):
        port = self.server.server_address[1]
        if self.headers.get("Origin") not in (None, f"http://127.0.0.1:{port}", f"http://localhost:{port}"):
            return self.reply({"error": "Origin not allowed"}, 403)
        try:
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 <= size <= 4096:
                raise ValueError("Invalid body length")
            data = json.loads(self.rfile.read(size) or b"{}")
            if not isinstance(data, dict):
                raise ValueError("Expected a JSON object")
            robot = self.server.robot
            if self.path == "/start":
                self.reply(robot.start(data))
            elif self.path == "/stop":
                self.reply(robot.request_stop())
            elif self.path == "/trigger-ack":
                robot.acknowledge_trigger(data)
                self.reply({"accepted": True})
            elif self.path == "/reset":
                robot.reset()
                self.reply({"reset": True})
            else:
                self.reply({"error": "Not found"}, 404)
        except Conflict as exc:
            self.reply({"error": str(exc)}, 409)
        except (ValueError, KeyError, TypeError) as exc:
            self.reply({"error": str(exc)}, 400)
        except (OSError, EOFError, RuntimeError) as exc:
            self.reply({"error": str(exc)}, 503)


class RobotServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, robot, port=8766):
        self.robot = robot
        super().__init__(("127.0.0.1", port), Handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--port", type=int)
    parser.add_argument("--plc-port", type=int)
    parser.add_argument("--recipe", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    cfg = load_config(args.config)
    if args.plc_port is not None:
        cfg["plc"]["port"] = args.plc_port
    robot = Robot(cfg, load_recipe(args.recipe or cfg["robot"]["recipe"]), args.output or cfg["runtimeDir"])
    with RobotServer(robot, args.port if args.port is not None else cfg["robot"]["port"]) as server:
        print(f"Robot console ready on http://127.0.0.1:{server.server_address[1]}", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            robot.stop.set()
            if robot.thread:
                robot.thread.join(timeout=5)


if __name__ == "__main__":
    main()
