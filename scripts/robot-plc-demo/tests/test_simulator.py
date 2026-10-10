"""Real loopback TCP/HTTP tests. VisionPeer scripts replies; it is NOT an image engine."""
import copy
import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from modbus_wire import Client, receive
from plc_service import PlcServer
from robot_service import Robot, RobotServer
from simulator_config import load_config, load_recipe, trigger_plan, validate_recipe, recipe_contract


def until(predicate, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.01)
    raise AssertionError("Condition did not become true before timeout")


def serve(server):
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True)
    thread.start()
    return thread


def scripted_ack(ticket, **changes):
    identity = {key: ticket[key] for key in ("sn", "k", "recipeId", "productCode", "shotCount", "shotId", "poseId", "camera", "view", "ordinal")}
    identity.update(cycleId=f"scripted-{ticket['sn']}", recipeRevision="scripted-recipe", bundleId=None)
    identity.update(changes)
    return {"id": ticket["id"], "ok": True, "identity": identity, "evidence": {"scope": "scripted-test-peer"}}


class ModbusTests(unittest.TestCase):
    def setUp(self):
        self.server = PlcServer(0, unit=7)
        self.thread = serve(self.server)
        self.port = self.server.server_address[1]

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(2)

    def test_independent_clients_share_coils_and_abcd_registers(self):
        with Client(self.port, 7) as writer, Client(self.port, 7) as reader:
            writer.coil(10, True)
            writer.write_registers(100, [0x1234, 0x5678, 101, 4])
            self.assertEqual(reader.coils(10, 2), [True, False])
            high, low = reader.registers(100, 2)
            self.assertEqual((high << 16) | low, 0x12345678)
            writer.request(struct.pack(">BHHBB", 15, 200, 5, 1, 0b10101))
            self.assertEqual(reader.coils(200, 5), [True, False, True, False, True])
            writer.request(struct.pack(">BHH", 6, 203, 77))
            self.assertEqual(reader.registers(203, 1), [77])
            self.assertEqual(reader.request(struct.pack(">BHH", 4, 203, 1)), b"\x04\x02\x00M")
            self.assertEqual(reader.request(struct.pack(">BHH", 2, 200, 5)), b"\x02\x01\x15")

    def test_fragmented_and_pipelined_tcp_packets(self):
        with socket.create_connection(("127.0.0.1", self.port), timeout=2) as sock:
            packet = struct.pack(">HHHBBHH", 72, 0, 6, 7, 6, 201, 1234)
            for offset in range(0, len(packet), 2):
                sock.sendall(packet[offset:offset + 2])
            self.assertEqual(receive(sock, 12), packet)
            packet = struct.pack(">HHHBBHH", 73, 0, 6, 7, 3, 201, 1)
            sock.sendall(packet + packet)
            for _ in range(2):
                self.assertEqual(receive(sock, 7), struct.pack(">HHHB", 73, 0, 5, 7))
                self.assertEqual(receive(sock, 4), b"\x03\x02\x04\xd2")

    def test_bad_pdu_has_exception_and_preserves_memory(self):
        with Client(self.port, 7) as client:
            client.write_registers(100, [44, 55])
            for pdu, error in [(struct.pack(">BHH", 3, 255, 2), "8302"),
                               (struct.pack(">BHH", 3, 100, 0), "8303"),
                               (struct.pack(">BHH", 5, 10, 123), "8503"),
                               (struct.pack(">BHHB", 16, 100, 2, 4) + b"\x00\x01", "9003"),
                               (b"\x63", "e301")]:
                with self.subTest(pdu=pdu.hex()), self.assertRaisesRegex(RuntimeError, error):
                    client.request(pdu)
            self.assertEqual(client.registers(100, 2), [44, 55])
        with Client(self.port, 1) as wrong_unit:
            with self.assertRaisesRegex(RuntimeError, "830b"):
                wrong_unit.registers(100, 1)

    def test_invalid_mbap_is_disconnected_and_server_survives(self):
        with socket.create_connection(("127.0.0.1", self.port), timeout=2) as sock:
            sock.sendall(struct.pack(">HHHB", 1, 1, 6, 7))
            self.assertEqual(sock.recv(1), b"")
        with Client(self.port, 7) as client:
            self.assertEqual(client.coils(20, 4), [False] * 4)

    def test_idle_client_survives_but_incomplete_packet_times_out(self):
        self.server.frame_timeout = 0.05
        with Client(self.port, 7) as client:
            client.coil(10, True)
            time.sleep(0.1)
            self.assertTrue(client.coils(10, 1)[0])
        with socket.create_connection(("127.0.0.1", self.port), timeout=2) as sock:
            sock.sendall(b"\x00")
            self.assertEqual(sock.recv(1), b"")


class VisionPeer:
    """Test-only peer: explicitly scripts result registers and camera acknowledgements."""
    def __init__(self, case, wrong_sn=False, stop_ready_after_one=False):
        self.case, self.wrong_sn = case, wrong_sn
        self.stop_ready_after_one = stop_ready_after_one
        self.stop = threading.Event()
        self.errors, self.triggers = [], []
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        try:
            with Client(self.case.port, 7) as client:
                client.coil(20, True)
                active = None
                scenario = "normal"
                replied = False
                while not self.stop.is_set():
                    flags = client.coils(10, 14)
                    if flags[0] and active is None:
                        high, low, product, shots = client.registers(100, 4)
                        if product != self.case.recipe["productCode"]:
                            raise AssertionError("Unexpected product code")
                        active = (high << 16) | low
                        scenario = "countMismatch" if shots != len(self.case.recipe["shots"]) else "normal"
                        if scenario != "countMismatch":
                            client.coil(21, True)
                            client.coil(22, True)
                    ticket = self.case.request("/trigger")[1]
                    if ticket:
                        if ticket["sn"] != active:
                            raise AssertionError("Trigger SN differs from PLC input")
                        self.triggers.append(ticket)
                        scenario = ticket["scenario"]
                        status, reply = self.case.request("/trigger-ack", scripted_ack(ticket))
                        if status != 200:
                            raise AssertionError(reply)
                    if active and not replied and (flags[1] or scenario == "countMismatch"):
                        code, fault = {"normal": (1, 0), "gap": (13, 0), "lostFrame": (90, 91),
                                       "locateFail": (90, 99), "countMismatch": (90, 94)}[scenario]
                        result_sn = active + 1 if self.wrong_sn else active
                        client.write_registers(110, [code, fault, result_sn >> 16, result_sn & 65535])
                        client.coil(21, False)
                        client.coil(22, False)
                        client.coil(23, True)
                        replied = True
                    if active and flags[2]:
                        client.coil(23, False)
                    if active and replied and not flags[0]:
                        if self.stop_ready_after_one:
                            client.coil(20, False)
                        active, replied = None, False
                    self.stop.wait(0.005)
        except Exception as exc:
            self.errors.append(exc)

    def close(self):
        self.stop.set()
        self.thread.join(3)


class RobotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.plc = PlcServer(0, unit=7)
        self.plc_thread = serve(self.plc)
        self.port = self.plc.server_address[1]
        self.cfg = load_config()
        self.cfg["plc"].update(port=self.port, unitId=7)
        self.cfg["robot"].update(motionSeconds=0.01, settleSeconds=0.02, releaseSeconds=0.05, intervalSeconds=0.1)
        self.cfg["robot"]["timeouts"] = dict.fromkeys(("ready", "armed", "trigger", "result", "release"), 0.8)
        self.recipe = load_recipe(self.cfg["robot"]["recipe"])
        self.robot = Robot(self.cfg, self.recipe, self.temp.name)
        self.http = RobotServer(self.robot, 0)
        self.http_thread = serve(self.http)
        self.url = f"http://127.0.0.1:{self.http.server_address[1]}"
        self.peer = None

    def tearDown(self):
        self.robot.stop.set()
        if self.robot.thread:
            self.robot.thread.join(3)
        if self.peer:
            self.peer.close()
        self.http.shutdown()
        self.http.server_close()
        self.plc.shutdown()
        self.plc.server_close()
        self.http_thread.join(2)
        self.plc_thread.join(2)
        self.temp.cleanup()

    def request(self, path, data=None, origin=None):
        headers = {"Content-Type": "application/json"}
        if origin:
            headers["Origin"] = origin
        request = Request(self.url + path, data=None if data is None else json.dumps(data).encode(), headers=headers)
        try:
            # Allow the Robot's bounded 2 s PLC connection timeout to be reported.
            response = urlopen(request, timeout=5)
        except HTTPError as exc:
            response = exc
        with response:
            return response.status, json.load(response)

    def start_peer(self, **options):
        self.peer = VisionPeer(self, **options)
        until(lambda: self.request("/state")[1]["bridgeOnline"])

    def finished(self):
        state = self.request("/state")[1]
        return state if not state["running"] else None

    def test_five_scenarios_over_real_http_and_modbus(self):
        self.start_peer()
        expected = {"normal": (1, 0), "gap": (13, 0), "lostFrame": (90, 91),
                    "locateFail": (90, 99), "countMismatch": (90, 94)}
        sns = []
        for scenario, codes in expected.items():
            with self.subTest(scenario=scenario):
                status, started = self.request("/start", {"scenario": scenario})
                self.assertEqual(status, 200)
                state = until(self.finished)
                self.assertEqual(state["phase"], "complete", state)
                result = state["results"][-1]
                self.assertEqual((result["resultCode"], result["faultCode"]), codes)
                self.assertEqual(result["runId"], started["runId"])
                self.assertEqual(result["sn"], result["resultSn"])
                sns.append(result["sn"])
                for flag in ("partStart", "partEnd", "resultAck", "armed", "busy", "done"):
                    self.assertFalse(state["plc"][flag], flag)
        self.assertEqual(len(set(sns)), 5)
        self.assertEqual([t["k"] for t in self.peer.triggers], [0, 1, 2, 3] * 4)
        self.assertEqual([t["view"] for t in self.peer.triggers], [1, 2, 3, 1] * 4)
        self.assertEqual(state["results"][0]["deviceTriggers"], {"cam1": 4})
        self.assertEqual(len(state["results"][0]["triggerAcks"]), 4)
        self.assertEqual(state["results"][-1]["deviceTriggers"], {"cam1": 0})
        self.assertEqual(self.peer.errors, [])

    def test_single_view_and_three_device_fixtures_keep_device_counts(self):
        self.start_peer()
        root = Path(__file__).resolve().parents[1] / "fixtures"
        for name, expected in (("ROBOT-DEMO", {"cam1": 4}), ("ROBOT-DEMO-3CAM", {"cam1": 2, "cam2": 1, "cam3": 1})):
            with self.subTest(recipe=name):
                self.recipe = load_recipe(root / f"{name}.json")
                self.robot = Robot(self.cfg, self.recipe, Path(self.temp.name) / name)
                self.http.robot = self.robot
                until(lambda: self.request("/state")[1]["bridgeOnline"])
                self.assertEqual(self.request("/start", {})[0], 200)
                state = until(self.finished)
                self.assertEqual(state["phase"], "complete", state)
                self.assertEqual(state["results"][-1]["deviceTriggers"], expected)
                self.assertEqual([a["identity"]["ordinal"] for a in state["results"][-1]["triggerAcks"]],
                                 [1, 1, 1, 2] if len(expected) == 3 else [1, 2, 3, 4])
        self.assertEqual(self.peer.errors, [])

    def test_count_and_restart_restore_results_without_reusing_sn(self):
        self.start_peer()
        self.assertEqual(self.request("/start", {"count": 2})[0], 200)
        state = until(self.finished)
        self.assertEqual(state["parts"], 2)
        restored = Robot(self.cfg, self.recipe, self.temp.name)
        self.assertEqual(restored.state["results"], state["results"])
        self.assertEqual(restored.state["parts"], 2)
        self.assertGreaterEqual(restored.last_sn, max(r["sn"] for r in state["results"]))

    def test_stop_drains_current_part_then_releases_handshake(self):
        self.start_peer()
        self.cfg["robot"]["motionSeconds"] = 0.12
        self.request("/start", {"continuous": True})
        until(lambda: self.request("/state")[1]["phase"] == "moving")
        status, stopped = self.request("/stop", {})
        self.assertEqual(status, 200)
        self.assertTrue(stopped["stopping"])
        waiting = self.request("/state")[1]
        self.assertTrue(waiting["running"])
        self.assertTrue(waiting["stopping"])
        self.assertFalse(waiting["canStart"])
        self.assertFalse(waiting["canReset"])
        self.assertEqual(self.request("/stop", {})[1], stopped)
        self.assertEqual(self.request("/start", {})[0], 409)
        state = until(self.finished)
        self.assertEqual(state["phase"], "stopped")
        self.assertFalse(state["stopping"])
        self.assertEqual(len([event for event in state["events"] if event["phase"] == "stopping"]), 1)
        self.assertEqual(state["parts"], 1)
        self.assertEqual(state["results"][0]["resultCode"], 1)
        for signal in ("partStart", "partEnd", "resultAck", "armed", "busy", "done"):
            self.assertFalse(state["plc"][signal], signal)
        self.assertTrue(state["canStart"])

    def test_wrong_result_sn_cannot_be_counted_as_completed(self):
        self.start_peer(wrong_sn=True)
        self.request("/start", {})
        state = until(self.finished)
        self.assertEqual(state["phase"], "error")
        self.assertIn("SN 不匹配", state["message"])
        self.assertEqual(state["parts"], 0)
        self.assertFalse(state["plc"]["resultAck"])
        self.assertFalse(state["plc"]["partStart"])

    def test_bridge_and_concurrent_start_guards(self):
        self.assertEqual(self.request("/start", {})[0], 409)
        self.cfg["robot"]["motionSeconds"] = 0.1
        self.start_peer()
        self.assertEqual(self.request("/start", {})[0], 200)
        self.assertEqual(self.request("/start", {})[0], 409)
        self.assertEqual(self.request("/reset", {})[0], 409)
        self.request("/stop", {})
        state = until(self.finished)
        self.assertIn(state["parts"], (0, 1))  # Stop may arrive before the first C10 write.
        self.assertEqual(state["phase"], "stopped")

    def test_not_ready_rejects_start_without_waiting_or_changing_inputs(self):
        self.request("/trigger")
        status, failure = self.request("/start", {})
        self.assertEqual(status, 409)
        self.assertIn("视觉尚未就绪", failure["error"])
        state = self.request("/state")[1]
        self.assertFalse(state["canStart"])
        self.assertFalse(state["running"])
        self.assertIsNone(state["runId"])
        self.assertIsNone(state["sn"])
        self.assertEqual(state["phase"], "idle")
        self.assertFalse(state["plc"]["partStart"])

    def test_ready_timeout_between_continuous_parts_leaves_inputs_clear(self):
        self.start_peer(stop_ready_after_one=True)
        self.assertEqual(self.request("/start", {"count": 2})[0], 200)
        state = until(self.finished)
        self.assertEqual(state["phase"], "error")
        self.assertIn("视觉未就绪", state["message"])
        self.assertFalse(state["plc"]["partStart"])
        self.assertEqual(state["parts"], 1)

    def test_full_handshake_must_be_idle_even_when_vision_ready(self):
        self.request("/trigger")
        with Client(self.port, 7) as client:
            client.coil(20, True)
            self.assertTrue(self.request("/state")[1]["canStart"])
            for address in (10, 11, 12, 13, 21, 22, 23):
                with self.subTest(address=address):
                    client.coil(address, True)
                    state = self.request("/state")[1]
                    self.assertFalse(state["canStart"])
                    self.assertTrue(state["startBlockedReason"])
                    status, result = self.request("/start", {})
                    self.assertEqual(status, 409)
                    self.assertEqual(result["error"], state["startBlockedReason"])
                    self.assertFalse(self.robot.state["running"])
                    self.assertIsNone(self.robot.state["runId"])
                    self.assertTrue(client.coils(address, 1)[0])
                    client.coil(address, False)

    def test_bridge_disconnect_and_plc_offline_disable_applicable_operations(self):
        with Client(self.port, 7) as client:
            client.coil(20, True)
        state = self.request("/state")[1]
        self.assertFalse(state["canStart"])
        self.assertTrue(state["canReset"])
        self.assertIn("触发桥离线", state["startBlockedReason"])
        self.request("/trigger")
        self.assertTrue(self.request("/state")[1]["canStart"])
        self.robot.bridge_seen = float("-inf")
        self.assertFalse(self.request("/state")[1]["canStart"])
        self.assertEqual(self.request("/start", {})[0], 409)
        self.request("/trigger")
        with socket.socket() as unused:
            unused.bind(("127.0.0.1", 0))  # Reserved but not listening.
            self.cfg["plc"]["port"] = unused.getsockname()[1]
            state = self.request("/state")[1]
            self.assertFalse(state["canStart"])
            self.assertFalse(state["canReset"])
            self.assertIn("PLC 服务未连接", state["startBlockedReason"])
            self.assertEqual(self.request("/start", {})[0], 409)
            self.assertFalse(self.robot.state["running"])

    def test_idle_stop_is_idempotent_and_does_not_report_a_pending_stop(self):
        for _ in range(2):
            status, response = self.request("/stop", {})
            self.assertEqual(status, 200)
            self.assertFalse(response["stopping"])
        state = self.request("/state")[1]
        self.assertFalse(state["stopping"])
        self.assertFalse(state["running"])
        self.assertEqual(state["events"], [])

    def test_reset_acknowledges_request_without_claiming_vision_is_ready(self):
        self.request("/trigger")
        with Client(self.port, 7) as client:
            client.coil(10, True)
            self.robot.event("error", "先前的运行错误")
            self.assertEqual(self.request("/reset", {})[0], 200)
            self.assertEqual(client.coils(10, 4), [False] * 4)
            state = self.request("/state")[1]
            self.assertEqual(state["phase"], "reset_requested")
            self.assertIn("故障复位请求已发送", state["message"])
            self.assertFalse(state["canStart"])
            client.coil(20, True)
            self.assertTrue(self.request("/state")[1]["canStart"])

    def test_console_and_module_are_served_without_caching(self):
        for path, content_type in (("/", "text/html"), ("/console.mjs", "text/javascript")):
            with self.subTest(path=path), urlopen(self.url + path, timeout=2) as response:
                self.assertEqual(response.status, 200)
                self.assertEqual(response.headers.get_content_type(), content_type)
                self.assertEqual(response.headers["Cache-Control"], "no-store")
                self.assertTrue(response.read())

    def test_missing_trigger_ack_is_reported_and_not_counted(self):
        self.request("/trigger")
        with Client(self.port, 7) as client:
            client.coil(20, True)
            self.request("/start", {})
            until(lambda: client.coils(10, 1)[0])
            client.coil(21, True)
            ticket = until(lambda: self.request("/trigger")[1])
            self.assertEqual(ticket["k"], 0)
            self.assertEqual(self.request("/trigger-ack", {"id": "stale", "ok": True})[0], 409)
            self.assertEqual(self.request("/trigger-ack", {"id": ticket["id"], "ok": True})[0], 400)
            self.assertEqual(self.request("/trigger-ack", scripted_ack(ticket, view=3))[0], 400)
            self.request("/stop", {})
            state = until(self.finished)
            self.assertEqual(state["phase"], "error")
            self.assertFalse(state["stopping"])
            self.assertFalse(any(event["phase"] == "stopped" for event in state["events"]))
            self.assertIn("触发桥未响应", state["message"])
            self.assertEqual(state["parts"], 0)
            self.assertFalse(state["plc"]["partStart"])

    def test_changed_cycle_ack_and_negative_ack_cannot_produce_a_completed_part(self):
        self.request("/trigger")
        with Client(self.port, 7) as client:
            client.coil(20, True)
            self.assertEqual(self.request("/start", {})[0], 200)
            until(lambda: client.coils(10, 1)[0])
            client.coil(21, True)
            first = until(lambda: self.request("/trigger")[1])
            self.assertEqual(self.request("/trigger-ack", scripted_ack(first))[0], 200)
            second = until(lambda: (ticket if (ticket := self.request("/trigger")[1]) and ticket["k"] == 1 else None))
            self.assertEqual(self.request("/trigger-ack", scripted_ack(second, cycleId="previous-cycle"))[0], 409)
            self.assertEqual(self.request("/trigger-ack", {"id": second["id"], "ok": False, "error": "scripted camera rejection"})[0], 200)
            state = until(self.finished)
            self.assertEqual(state["phase"], "error")
            self.assertIn("scripted camera rejection", state["message"])
            self.assertEqual(state["parts"], 0)
            self.assertFalse(state["plc"]["resultAck"])

    def test_plc_write_refusal_is_reported_and_a_later_run_recovers(self):
        self.start_peer()
        original = self.plc.memory.dispatch
        def reject_part_registers(pdu):
            return b"\x90\x04" if pdu[0] == 16 and pdu[1:3] == b"\x00\x64" else original(pdu)
        with patch.object(self.plc.memory, "dispatch", side_effect=reject_part_registers):
            self.assertEqual(self.request("/start", {})[0], 200)
            state = until(self.finished)
        self.assertEqual(state["phase"], "error")
        self.assertIn("Modbus exception", state["message"])
        self.assertEqual(state["parts"], 0)
        self.assertFalse(state["plc"]["partStart"])
        self.assertEqual(self.request("/start", {})[0], 200)
        self.assertEqual(until(self.finished)["parts"], 1)

    def test_result_disk_error_never_increments_completion(self):
        self.start_peer()
        result_path = Path(self.temp.name) / "robot-results.jsonl"
        result_path.mkdir()
        self.assertEqual(self.request("/start", {})[0], 200)
        state = until(self.finished)
        self.assertEqual(state["phase"], "error")
        self.assertEqual(state["parts"], 0)
        self.assertEqual(state["results"], [])
        self.assertFalse(any(state["plc"][key] for key in ("partStart", "partEnd", "resultAck", "done")))
        result_path.rmdir()
        self.assertEqual(self.request("/start", {})[0], 200)
        self.assertEqual(until(self.finished)["parts"], 1)

    def test_bad_http_input_and_origin_have_no_side_effect(self):
        for data in ([], {"count": 0}, {"count": True}, {"scenario": "unknown"}, {"continuous": "false"}):
            with self.subTest(data=data):
                self.assertEqual(self.request("/start", data)[0], 400)
        self.assertEqual(self.request("/start", {}, origin="https://example.com")[0], 403)
        self.assertEqual(self.request("/stop", {}, origin=self.url)[0], 200)
        self.assertFalse(self.robot.state["running"])

    def test_regression_preflight_failure_replaces_stale_pass_report(self):
        cfg = copy.deepcopy(self.cfg)
        cfg["robot"]["port"] = self.http.server_address[1]
        cfg["runtimeDir"] = self.temp.name
        path = Path(self.temp.name) / "test.config.json"
        path.write_text(json.dumps(cfg), encoding="utf-8")
        report_path = Path(self.temp.name) / "regression-latest.json"
        report_path.write_text('{"passed": true}', encoding="utf-8")
        result = subprocess.run([sys.executable, str(Path(__file__).resolve().parents[1] / "run-regression.py"),
                                 "--config", str(path)], capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertFalse(report["passed"])
        self.assertIn("Requires an idle", report["error"])
        self.assertFalse(self.robot.state["running"])

    def test_interrupted_final_result_is_backed_up_and_repaired(self):
        path = Path(self.temp.name) / "robot-results.jsonl"
        path.write_text('{"sn":123,"resultCode":1,"faultCode":0}\n{"sn":', encoding="utf-8")
        restored = Robot(self.cfg, self.recipe, self.temp.name)
        self.assertEqual(restored.state["parts"], 1)
        self.assertEqual(json.loads(path.read_text())["sn"], 123)
        self.assertEqual(len(list(path.parent.glob("robot-results.interrupted-*.jsonl"))), 1)


class RecipeContractVersionTests(unittest.TestCase):
    def test_missing_and_zero_versions_match_actual_backend_version_one(self):
        recipe = load_recipe(load_config()["robot"]["recipe"])
        recipe.pop("version", None)
        validate_recipe(recipe)
        absent = recipe_contract(recipe)
        recipe["version"] = 0
        validate_recipe(recipe)
        self.assertEqual(absent, recipe_contract(recipe))
        self.assertEqual(absent["version"], 1)
        recipe["version"] = 1
        self.assertEqual(absent, recipe_contract(recipe))
        recipe["version"] = 7
        validate_recipe(recipe)
        self.assertEqual(recipe_contract(recipe)["version"], 7)


    def test_invalid_versions_remain_rejected_by_validator_and_contract(self):
        recipe = load_recipe(load_config()["robot"]["recipe"])
        for version in [None, True, False, -1, 0.5, float("nan"), float("inf"), 0x100000000, "1"]:
            with self.subTest(version=version):
                recipe["version"] = version
                with self.assertRaises(ValueError):
                    validate_recipe(recipe)
                with self.assertRaises(ValueError):
                    recipe_contract(recipe)

class ConfigurationTests(unittest.TestCase):
    def test_schema_four_fixtures_and_per_device_ordinals(self):
        root = Path(__file__).resolve().parents[1] / "fixtures"
        expected = {"ROBOT-DEMO": ([1, 1, 1, 1], {"cam1": 4}),
                    "ROBOT-DEMO-TRICAM": ([1, 2, 3, 1], {"cam1": 4}),
                    "ROBOT-DEMO-3CAM": ([1, 1, 1, 1], {"cam1": 2, "cam2": 1, "cam3": 1})}
        for name, (views, counts) in expected.items():
            with self.subTest(recipe=name):
                recipe = load_recipe(root / f"{name}.json")
                plan, devices = trigger_plan(recipe)
                self.assertEqual([p["view"] for p in plan], views)
                self.assertEqual(devices, counts)
                self.assertEqual([p["ordinal"] for p in plan], [1, 1, 1, 2] if len(counts) == 3 else [1, 2, 3, 4])

    def test_legacy_geometry_and_invalid_per_shot_contracts_are_rejected(self):
        recipe = load_recipe(load_config()["robot"]["recipe"])
        edits = [lambda r: r.update(schemaVersion=3), lambda r: r.update(schemaVersion=4.0),
                 lambda r: r.update(shots=[[95, 50]]), lambda r: r["shots"][0].update(view=0),
                 lambda r: r["shots"][0].update(view=4), lambda r: r["shots"][0].update(view=True),
                 lambda r: r["shots"][0].pop("view"), lambda r: r["shots"][1].update(id="P1"),
                 lambda r: r["shots"][0].update(poseId=""), lambda r: r["shots"][0].update(camera="../cam1"),
                 lambda r: r["shots"][0].update(path=[[1, float("inf")], [2, 3]]),
                 lambda r: r["shots"][0].update(mmPerPx=None),
                 lambda r: r["shots"][0].update(detect={"searchMm": 2, "widthRange": [1, 5], "polarity": "dark"}),
                 lambda r: r["shots"][0]["limits"].update(maxGapLen=-1)]
        for edit in edits:
            invalid = copy.deepcopy(recipe)
            edit(invalid)
            with self.subTest(recipe=invalid), self.assertRaises(ValueError):
                validate_recipe(invalid)

    def test_config_paths_are_independent_of_current_directory_and_bad_config_fails(self):
        cfg = load_config()
        self.assertTrue(Path(cfg["robot"]["recipe"]).is_absolute())
        self.assertEqual(len(load_recipe(cfg["robot"]["recipe"])["shots"]), 4)
        raw = json.loads(Path(cfg["configPath"]).read_text())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "custom.json"
            for edit in (lambda c: c.update(schemaVersion=2),
                         lambda c: c["plc"].update(port=c["robot"]["port"]),
                         lambda c: c["robot"].update(motionSeconds=float("nan")),
                         lambda c: c["plc"].update(unitId=0)):
                invalid = copy.deepcopy(raw)
                edit(invalid)
                path.write_text(json.dumps(invalid), encoding="utf-8")
                with self.assertRaises(ValueError):
                    load_config(path)


if __name__ == "__main__":
    unittest.main()
