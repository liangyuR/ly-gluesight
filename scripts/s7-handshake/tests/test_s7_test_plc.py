import importlib.util
import json
from pathlib import Path
import queue
import socket
import subprocess
import sys
import threading
import time
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "s7_test_plc.py"
SPEC = importlib.util.spec_from_file_location("s7_test_plc", SCRIPT)
PLC = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PLC
SPEC.loader.exec_module(PLC)

COTP_CR = bytes.fromhex("03000016 11e0 0000 0001 00 c0010a c1020100 c2020102")
COTP_CC = bytes.fromhex("03000016 11d0 0001 0001 00 c0010a c1020102 c2020100")
SETUP = bytes.fromhex("03000019 02f080 32010000 0001 0008 0000 f0000001000101e0")
SETUP_ACK = bytes.fromhex("0300001b 02f080 32030000 0001 0008 0000 0000 f0000001000101e0")
READ_BYTES = bytes.fromhex("0300001f 02f080 32010000 0002 000e 0000 0401 120a10020008006484000020")
WRITE_BYTES = bytes.fromhex("03000027 02f080 32010000 0003 000e 0008 0501 120a10020004006484000020 0004002000000007")
WRITE_ACK = bytes.fromhex("03000016 02f080 32030000 0003 0002 0001 0000 0501ff")
WRITE_DONE = bytes.fromhex("03000024 02f080 32010000 0004 000e 0005 0501 120a10010001006484000203 0003000101")
DONE_ACK = bytes.fromhex("03000016 02f080 32030000 0004 0002 0001 0000 0501ff")
READ_DONE = bytes.fromhex("0300001f 02f080 32010000 0005 000e 0000 0401 120a10010001006484000203")
READ_DONE_ACK = bytes.fromhex("0300001a 02f080 32030000 0005 0002 0005 0000 0401ff03000101")
READ_MULTI = bytes.fromhex("0300002b 02f080 32010000 0006 001a 0000 0402 120a10020001006484000020 120a10020003006484000040")
WRITE_MULTI = bytes.fromhex("03000036 02f080 32010000 0007 001a 000b 0502 120a10020001006484000020 120a10020001006484000040 00040008a100 00040008b2")


def receive_packet(connection):
    header = b""
    while len(header) < 4:
        chunk = connection.recv(4 - len(header))
        if not chunk:
            raise EOFError("server disconnected before a complete TPKT header")
        header += chunk
    if header[:2] != b"\x03\x00":
        raise AssertionError(f"invalid TPKT header: {header.hex()}")
    remaining = int.from_bytes(header[2:4], "big") - 4
    payload = b""
    while len(payload) < remaining:
        chunk = connection.recv(remaining - len(payload))
        if not chunk:
            raise EOFError("server disconnected inside a TPKT packet")
        payload += chunk
    return header + payload


def exchange(connection, packet):
    connection.sendall(packet)
    return receive_packet(connection)


def connect(address):
    connection = socket.create_connection(address, timeout=2)
    if exchange(connection, COTP_CR) != COTP_CC:
        connection.close()
        raise AssertionError("COTP connection confirm differs from the golden packet")
    if exchange(connection, SETUP) != SETUP_ACK:
        connection.close()
        raise AssertionError("S7 setup acknowledgement differs from the golden packet")
    return connection


class S7WireTests(unittest.TestCase):
    def setUp(self):
        self.state = PLC.PlcState()
        self.state.heartbeat_enabled = False
        self.server = PLC.S7Server(self.state).start()
        self.addCleanup(self.server.stop)
        self.connection = connect(self.server.server_address)
        self.addCleanup(self.connection.close)

    def test_byte_read_write_matches_fixed_wire_packets(self):
        self.assertEqual(exchange(self.connection, WRITE_BYTES), WRITE_ACK)
        self.state.control({"op": "write_db", "byte": 8, "hex": "12345678"})
        expected = bytes.fromhex("03000021 02f080 32030000 0002 0002 000c 0000 0401 ff040040 0000000712345678")
        self.assertEqual(exchange(self.connection, READ_BYTES), expected)
        self.assertEqual(self.state.field_values()["requestSeq"], 7)
        self.assertEqual(self.state.field_values()["partSn"], 0x12345678)

    def test_bit_write_preserves_neighboring_bits(self):
        self.state.control({"op": "write_db", "byte": 64, "hex": "81"})
        self.assertEqual(exchange(self.connection, WRITE_DONE), DONE_ACK)
        self.assertEqual(self.state.control({"op": "read_db", "byte": 64})["hex"], "89")
        self.assertEqual(exchange(self.connection, READ_DONE), READ_DONE_ACK)
        self.assertEqual(exchange(self.connection, WRITE_DONE[:-1] + b"\x00"), DONE_ACK)
        self.assertEqual(self.state.control({"op": "read_db", "byte": 64})["hex"], "81")

    def test_multi_item_read_uses_byte_padding_and_bit_lengths(self):
        self.state.control({"op": "write_db", "byte": 4, "hex": "a1"})
        self.state.control({"op": "write_db", "byte": 8, "hex": "b2c3d4"})
        expected = bytes.fromhex("03000022 02f080 32030000 0006 0002 000d 0000 0402 ff040008a100 ff040018b2c3d4")
        self.assertEqual(exchange(self.connection, READ_MULTI), expected)

    def test_multi_item_write_and_targeted_rejection(self):
        self.state.control({"op": "fault", "function": "write", "kind": "reject", "db": 100, "byte": 8})
        rejected = bytes.fromhex("03000017 02f080 32030000 0007 0002 0002 0000 0502ff03")
        self.assertEqual(exchange(self.connection, WRITE_MULTI), rejected)
        self.assertEqual(self.state.control({"op": "read_db", "byte": 4})["hex"], "a1")
        self.assertEqual(self.state.control({"op": "read_db", "byte": 8})["hex"], "00")
        accepted = bytes.fromhex("03000017 02f080 32030000 0007 0002 0002 0000 0502ffff")
        self.assertEqual(exchange(self.connection, WRITE_MULTI), accepted)
        self.assertEqual(self.state.control({"op": "read_db", "byte": 8})["hex"], "b2")

    def test_unknown_db_and_out_of_bounds_have_s7_item_errors(self):
        unknown = READ_BYTES.replace(bytes.fromhex("006484000020"), bytes.fromhex("006384000020"))
        self.assertEqual(exchange(self.connection, unknown), bytes.fromhex("03000019 02f080 32030000 0002 0002 0004 0000 04010a000000"))
        outside = READ_BYTES[:-3] + bytes.fromhex("002000")
        self.assertEqual(exchange(self.connection, outside), bytes.fromhex("03000019 02f080 32030000 0002 0002 0004 0000 040105000000"))

    def test_read_rejection_is_counted_and_retry_succeeds(self):
        self.state.control({"op": "fault", "function": "read", "kind": "reject", "count": 2})
        expected = bytes.fromhex("03000019 02f080 32030000 0002 0002 0004 0000 040103000000")
        self.assertEqual(exchange(self.connection, READ_BYTES), expected)
        self.assertEqual(exchange(self.connection, READ_BYTES), expected)
        self.assertEqual(exchange(self.connection, READ_BYTES)[21], 0xFF)
        self.assertEqual(self.state.control({"op": "status"})["counts"]["faults"], 2)

    def test_write_rejection_does_not_mutate_db_or_get_consumed_by_reads(self):
        self.state.control({"op": "fault", "function": "write", "kind": "reject", "byte": 4})
        exchange(self.connection, READ_BYTES)
        self.assertEqual(exchange(self.connection, WRITE_BYTES), WRITE_ACK[:-1] + b"\x03")
        self.assertEqual(self.state.field_values()["requestSeq"], 0)
        self.assertEqual(exchange(self.connection, WRITE_BYTES), WRITE_ACK)

    def test_delay_is_observable_by_a_socket_client(self):
        self.state.control({"op": "fault", "function": "read", "kind": "delay", "delay_ms": 80})
        started = time.monotonic()
        self.connection.sendall(READ_BYTES)
        self.connection.settimeout(0.02)
        with self.assertRaises(socket.timeout):
            self.connection.recv(1)
        self.connection.settimeout(2)
        self.assertEqual(receive_packet(self.connection)[21], 0xFF)
        self.assertGreaterEqual(time.monotonic() - started, 0.06)

    def test_disconnect_before_write_preserves_memory_and_allows_reconnect(self):
        self.state.control({"op": "fault", "function": "write", "kind": "disconnect"})
        self.connection.sendall(WRITE_BYTES)
        self.assertEqual(self.connection.recv(1), b"")
        self.assertEqual(self.state.field_values()["requestSeq"], 0)
        with connect(self.server.server_address) as connection:
            self.assertEqual(exchange(connection, WRITE_BYTES), WRITE_ACK)

    def test_disconnect_after_write_models_uncertain_write_outcome(self):
        self.state.control({"op": "fault", "function": "write", "kind": "disconnect", "after_apply": True})
        self.connection.sendall(WRITE_BYTES)
        self.assertEqual(self.connection.recv(1), b"")
        self.assertEqual(self.state.field_values()["requestSeq"], 7)
        with connect(self.server.server_address) as connection:
            self.assertEqual(exchange(connection, READ_BYTES)[25:29], b"\x00\x00\x00\x07")

    def test_fragmented_tcp_packets_and_two_requests_in_one_write(self):
        for value in WRITE_BYTES:
            self.connection.sendall(bytes([value]))
        self.assertEqual(receive_packet(self.connection), WRITE_ACK)
        self.connection.sendall(WRITE_DONE + READ_DONE)
        self.assertEqual(receive_packet(self.connection), DONE_ACK)
        self.assertEqual(receive_packet(self.connection), READ_DONE_ACK)

    def test_invalid_write_data_never_changes_memory(self):
        invalid = bytearray(WRITE_BYTES)
        invalid[33:35] = b"\x00\x18"
        response = exchange(self.connection, bytes(invalid))
        self.assertEqual(response, bytes.fromhex("03000013 02f080 32030000 0003 0000 0000 8104"))
        self.assertEqual(self.state.field_values()["requestSeq"], 0)

    def test_oversized_read_is_rejected_before_encoding_bit_length(self):
        request = READ_BYTES.replace(bytes.fromhex("100200080064"), bytes.fromhex("1002ffff0064"))
        self.assertEqual(exchange(self.connection, request), bytes.fromhex("03000013 02f080 32030000 0002 0000 0000 8500"))

    def test_malformed_tpkt_disconnects_only_its_client(self):
        self.connection.sendall(b"\x04\x00\x00\x07")
        self.assertEqual(self.connection.recv(1), b"")
        with connect(self.server.server_address) as connection:
            self.assertEqual(exchange(connection, WRITE_DONE), DONE_ACK)

    def test_read_requires_setup_negotiation(self):
        with socket.create_connection(self.server.server_address, timeout=2) as connection:
            self.assertEqual(exchange(connection, COTP_CR), COTP_CC)
            self.assertEqual(exchange(connection, READ_BYTES), bytes.fromhex("03000013 02f080 32030000 0002 0000 0000 8104"))

    def test_trace_contains_actual_wire_requests_and_responses(self):
        exchange(self.connection, WRITE_BYTES)
        packets = [event for event in self.state.control({"op": "trace"}) if event["event"] == "packet"]
        self.assertEqual(packets[-1]["request"], WRITE_BYTES.hex())
        self.assertEqual(packets[-1]["response"], WRITE_ACK.hex())
        self.assertEqual(packets[-1]["function"], "write")


class S7HeartbeatTests(unittest.TestCase):
    def test_heartbeat_freeze_resume_and_default_point_table(self):
        state = PLC.PlcState(db=321)
        state.heartbeat_ms = 15
        server = PLC.S7Server(state).start()
        try:
            self.assertEqual(state.field_values()["protocolVersion"], 1)
            self.assertEqual(state.fields["requestSeq"], {"db": 321, "byte": 4, "type": "u32"})
            self.assertEqual(state.fields["acceptedPlanHash"]["byte"], 84)
            deadline = time.monotonic() + 1
            while not state.field_values()["plcHeartbeat"] and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertTrue(state.field_values()["plcHeartbeat"])
            state.control({"op": "fault", "kind": "heartbeat_stop"})
            frozen = state.field_values()["plcHeartbeat"]
            time.sleep(0.06)
            self.assertEqual(state.field_values()["plcHeartbeat"], frozen)
            state.control({"op": "fault", "kind": "heartbeat_resume"})
            deadline = time.monotonic() + 1
            while state.field_values()["plcHeartbeat"] == frozen and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertNotEqual(state.field_values()["plcHeartbeat"], frozen)
        finally:
            server.stop()


class ProcessFixture:
    def __init__(self, *options):
        self.process = subprocess.Popen([sys.executable, "-u", str(SCRIPT), "--port", "0", *options], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        self.lines = queue.Queue()
        self.reader = threading.Thread(target=self.read_lines, daemon=True)
        self.reader.start()
        self.next_id = 0
        self.ready = self.receive()

    def read_lines(self):
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def receive(self):
        line = self.lines.get(timeout=5)
        if line is None:
            raise AssertionError("test PLC exited before its response")
        return json.loads(line)

    def command(self, op, **values):
        self.next_id += 1
        self.process.stdin.write(json.dumps({"id": self.next_id, "op": op, **values}) + "\n")
        self.process.stdin.flush()
        response = self.receive()
        if response.get("id") != self.next_id:
            raise AssertionError(f"control response id mismatch: {response}")
        return response

    def close(self):
        if self.process.poll() is None:
            self.command("shutdown")
        try:
            self.process.wait(timeout=5)
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=5)
            self.process.stdin.close()
            self.reader.join(timeout=1)
            self.process.stdout.close()
            errors = self.process.stderr.read()
            self.process.stderr.close()
        if self.process.returncode != 0 or errors:
            raise AssertionError(f"test PLC failed: exit={self.process.returncode}, stderr={errors}")


class S7ProcessApiTests(unittest.TestCase):
    def setUp(self):
        self.fixture = ProcessFixture()
        self.fixture.command("heartbeat", enabled=False)

    def tearDown(self):
        self.fixture.close()

    def test_ready_is_loopback_random_port_and_bad_commands_are_isolated(self):
        ready = self.fixture.ready
        self.assertEqual((ready["event"], ready["protocol"], ready["host"]), ("ready", "s7", "127.0.0.1"))
        self.assertGreater(ready["port"], 1024)
        self.assertFalse(self.fixture.command("write_db", byte=1024, hex="01")["ok"])
        self.assertFalse(self.fixture.command("set_bit", byte=0, bit=8, value=True)["ok"])
        self.assertFalse(self.fixture.command("fault", kind="delay", delay_ms=-1)["ok"])
        self.assertFalse(self.fixture.command("missing")["ok"])
        self.assertTrue(self.fixture.command("status")["ok"])

    def test_request_ack_release_observe_pc_s7_writes(self):
        response = self.fixture.command("plc_request", values={"requestSeq": 17, "partSn": 12345, "productCode": 7, "shotCount": 4, "camera1Shots": 2, "camera2Shots": 1, "camera3Shots": 1, "planVersion": 1, "planHash": 123})
        self.assertTrue(response["ok"])
        self.assertTrue(response["result"]["partStart"])
        self.assertEqual(response["result"]["requestSeq"], 17)
        with connect((self.fixture.ready["host"], self.fixture.ready["port"])) as connection:
            result_seq = bytes.fromhex("03000027 02f080 32010000 0003 000e 0008 0501 120a10020004006484000220 0004002000000011")
            self.assertEqual(exchange(connection, result_seq), WRITE_ACK)
            self.assertEqual(exchange(connection, WRITE_DONE), DONE_ACK)
            acknowledged = self.fixture.command("plc_ack")
            self.assertTrue(acknowledged["ok"])
            self.assertEqual(acknowledged["result"]["ackSeq"], 17)
            self.assertTrue(acknowledged["result"]["resultAck"])
            self.assertFalse(self.fixture.command("plc_release")["ok"])
            self.assertEqual(exchange(connection, WRITE_DONE[:-1] + b"\x00"), DONE_ACK)
        released = self.fixture.command("plc_release")["result"]
        self.assertFalse(released["partStart"])
        self.assertFalse(released["partEnd"])
        self.assertFalse(released["resultAck"])

    def test_field_update_validation_is_atomic(self):
        result = self.fixture.command("plc_request", values={"requestSeq": 99, "camera1Shots": -1})
        self.assertFalse(result["ok"])
        fields = self.fixture.command("status")["result"]["fields"]
        self.assertEqual(fields["requestSeq"], 0)
        self.assertFalse(fields["partStart"])

    def test_nondefault_db_is_readable_through_real_s7(self):
        fixture = ProcessFixture("--db", "321")
        try:
            self.assertEqual(fixture.ready["db"], 321)
            fixture.command("plc_request", values={"requestSeq": 7})
            with connect((fixture.ready["host"], fixture.ready["port"])) as connection:
                request = READ_BYTES.replace(bytes.fromhex("006484000020"), bytes.fromhex("014184000020"))
                self.assertEqual(exchange(connection, request)[25:29], b"\x00\x00\x00\x07")
        finally:
            fixture.close()


if __name__ == "__main__":
    unittest.main()
