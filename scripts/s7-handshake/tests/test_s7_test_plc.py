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
SETUP_ACK = bytes.fromhex("0300001b 02f080 32030000 0001 0008 0000 0000 f0000001000100f0")
READ_BYTES = bytes.fromhex("0300001f 02f080 32010000 0002 000e 0000 0401 120a10020008000484000040")
WRITE_BYTES = bytes.fromhex("03000027 02f080 32010000 0003 000e 0008 0501 120a10020004000484000040 0004002000000007")
WRITE_ACK = bytes.fromhex("03000016 02f080 32030000 0003 0002 0001 0000 0501ff")
WRITE_DONE = bytes.fromhex("03000024 02f080 32010000 0004 000e 0005 0501 120a10010001000284000003 0003000101")
DONE_ACK = bytes.fromhex("03000016 02f080 32030000 0004 0002 0001 0000 0501ff")
READ_DONE = bytes.fromhex("0300001f 02f080 32010000 0005 000e 0000 0401 120a10010001000284000003")
READ_DONE_ACK = bytes.fromhex("0300001a 02f080 32030000 0005 0002 0005 0000 0401ff03000101")
READ_MULTI = bytes.fromhex("0300002b 02f080 32010000 0006 001a 0000 0402 120a10020001000484000020 120a10020003000484000040")
WRITE_MULTI = bytes.fromhex("03000036 02f080 32010000 0007 001a 000b 0502 120a10020001000484000020 120a10020001000484000040 00040008a100 00040008b2")


def write_bytes_packet(reference, db, byte, data):
    item = bytes([0x12, 0x0A, 0x10, 0x02]) + len(data).to_bytes(2, "big") + db.to_bytes(2, "big") + b"\x84" + (byte * 8).to_bytes(3, "big")
    parameters = b"\x05\x01" + item
    payload = b"\x00\x04" + (len(data) * 8).to_bytes(2, "big") + data
    s7 = b"\x32\x01\x00\x00" + reference.to_bytes(2, "big") + len(parameters).to_bytes(2, "big") + len(payload).to_bytes(2, "big") + parameters + payload
    body = b"\x02\xf0\x80" + s7
    return b"\x03\x00" + (len(body) + 4).to_bytes(2, "big") + body


def read_bytes_packet(reference, db, byte, size):
    parameters = b"\x04\x01\x12\x0a\x10\x02" + size.to_bytes(2, "big") + db.to_bytes(2, "big") + b"\x84" + (byte * 8).to_bytes(3, "big")
    s7 = b"\x32\x01\x00\x00" + reference.to_bytes(2, "big") + len(parameters).to_bytes(2, "big") + b"\x00\x00" + parameters
    body = b"\x02\xf0\x80" + s7
    return b"\x03\x00" + (len(body) + 4).to_bytes(2, "big") + body


def words(*values):
    return b"".join(value.to_bytes(2, "big", signed=True) for value in values)


def s7_string(text):
    return bytes([50, len(text)]) + text.encode("ascii") + bytes(50 - len(text))


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

    def test_teaching_blocks_are_independent_and_failed_state_write_preserves_request(self):
        production = {db: bytes(self.state.dbs[db]) for db in (2, 4)}
        request = bytes.fromhex("0001000a000700000000")
        self.assertEqual(exchange(self.connection, write_bytes_packet(10, 6, 0, request))[-1], 0xff)
        self.assertEqual(self.state.field_values()["teachRoundId"], 7)
        self.assertEqual(self.state.field_values()["teachPcState"], 10)
        self.state.control({"op": "fault", "function": "write", "kind": "reject", "db": 6, "byte": 2})
        self.assertEqual(exchange(self.connection, write_bytes_packet(11, 6, 2, bytes.fromhex("0028")))[-1], 3)
        self.assertEqual(self.state.field_values()["teachPcState"], 10)
        self.state.control({"op": "plc_values", "values": {"teachRoundIdEcho": 7, "teachPlcState": 10, "teachPlcReason": 0}})
        read = READ_BYTES.replace(bytes.fromhex("0008000484000040"), bytes.fromhex("000a000c84000000"))
        self.assertEqual(exchange(self.connection, read)[-10:], bytes.fromhex("0001000a000700000000"))
        self.assertEqual({db: bytes(self.state.dbs[db]) for db in (2, 4)}, production)
        outside = READ_BYTES.replace(bytes.fromhex("0008000484000040"), bytes.fromhex("0002000c84000050"))
        self.assertEqual(exchange(self.connection, outside)[21], 5)

    def test_byte_read_write_matches_fixed_wire_packets(self):
        self.assertEqual(exchange(self.connection, WRITE_BYTES), WRITE_ACK)
        self.state.control({"op": "write_db", "byte": 12, "hex": "12345678"})
        expected = bytes.fromhex("03000021 02f080 32030000 0002 0002 000c 0000 0401 ff040040 0000000712345678")
        self.assertEqual(exchange(self.connection, READ_BYTES), expected)
        self.assertEqual(self.state.field_values()["requestSeq"], 7)

    def test_bit_write_preserves_neighboring_bits(self):
        self.state.control({"op": "write_db", "db": 2, "byte": 0, "hex": "81"})
        self.assertEqual(exchange(self.connection, WRITE_DONE), DONE_ACK)
        self.assertEqual(self.state.control({"op": "read_db", "db": 2, "byte": 0})["hex"], "89")
        self.assertEqual(exchange(self.connection, READ_DONE), READ_DONE_ACK)
        self.assertEqual(exchange(self.connection, WRITE_DONE[:-1] + b"\x00"), DONE_ACK)
        self.assertEqual(self.state.control({"op": "read_db", "db": 2, "byte": 0})["hex"], "81")

    def test_multi_item_read_uses_byte_padding_and_bit_lengths(self):
        self.state.control({"op": "write_db", "byte": 4, "hex": "a1"})
        self.state.control({"op": "write_db", "byte": 8, "hex": "b2c3d4"})
        expected = bytes.fromhex("03000022 02f080 32030000 0006 0002 000d 0000 0402 ff040008a100 ff040018b2c3d4")
        self.assertEqual(exchange(self.connection, READ_MULTI), expected)

    def test_multi_item_write_and_targeted_rejection(self):
        self.state.control({"op": "fault", "function": "write", "kind": "reject", "db": 4, "byte": 8})
        rejected = bytes.fromhex("03000017 02f080 32030000 0007 0002 0002 0000 0502ff03")
        self.assertEqual(exchange(self.connection, WRITE_MULTI), rejected)
        self.assertEqual(self.state.control({"op": "read_db", "byte": 4})["hex"], "a1")
        self.assertEqual(self.state.control({"op": "read_db", "byte": 8})["hex"], "00")
        accepted = bytes.fromhex("03000017 02f080 32030000 0007 0002 0002 0000 0502ffff")
        self.assertEqual(exchange(self.connection, WRITE_MULTI), accepted)
        self.assertEqual(self.state.control({"op": "read_db", "byte": 8})["hex"], "b2")

    def test_unknown_db_and_out_of_bounds_have_s7_item_errors(self):
        unknown = READ_BYTES.replace(bytes.fromhex("000484000040"), bytes.fromhex("006384000040"))
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
        self.state.control({"op": "fault", "function": "write", "kind": "reject", "byte": 8})
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
        request = READ_BYTES.replace(bytes.fromhex("100200080004"), bytes.fromhex("1002ffff0004"))
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

    def test_protocol7_db4_and_db2_offsets_match_point_table(self):
        self.state.control({"op": "plc_values", "values": {"plcHeartbeat": 3, "requestSeq": 0x01020304, "productCode": 7, "sn": "SN-1", "plcReason": 6, "plcState": 90}})
        response = exchange(self.connection, read_bytes_packet(8, 4, 0, 68))
        self.assertEqual(response[21], 0xFF)
        self.assertEqual(response[25:], words(7, 3, 90, 6) + bytes.fromhex("01020304") + words(7, 0) + s7_string("SN-1"))
        pc_area = words(7, 1, 30, 1) + bytes.fromhex("01020304") + words(90, 95) + s7_string("SN-1")
        self.assertEqual(exchange(self.connection, write_bytes_packet(3, 2, 0, pc_area)), WRITE_ACK)
        fields = self.state.field_values()
        self.assertEqual({name: fields[name] for name in ("pcVersion", "pcHeartbeat", "pcState", "pcMode", "seqEcho", "resultCode", "faultCode", "snEcho")},
                         {"pcVersion": 7, "pcHeartbeat": 1, "pcState": 30, "pcMode": 1, "seqEcho": 0x01020304, "resultCode": 90, "faultCode": 95, "snEcho": "SN-1"})


class S7HeartbeatTests(unittest.TestCase):
    def test_int_heartbeat_advances_freezes_and_resumes(self):
        state = PLC.PlcState()
        state.heartbeat_ms = 15
        server = PLC.S7Server(state).start()
        try:
            deadline = time.monotonic() + 1
            while state.field_values()["plcHeartbeat"] < 2 and time.monotonic() < deadline:
                time.sleep(0.005)
            self.assertGreaterEqual(state.field_values()["plcHeartbeat"], 2)
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


class S7SerialNumberTests(unittest.TestCase):
    EMPTY = "32" + "00" * 51

    def setUp(self):
        self.state = PLC.PlcState()
        self.state.heartbeat_enabled = False

    def read(self, db, byte, size=52):
        return self.state.control({"op": "read_db", "db": db, "byte": byte, "size": size})["hex"]

    def test_fresh_db_sn_fields_are_empty_s7_strings(self):
        self.assertEqual(self.read(4, 16), self.EMPTY)
        self.assertEqual(self.read(2, 16), self.EMPTY)

    def test_shorter_sn_after_longest_sn_writes_new_length_and_zero_padding(self):
        full = "ABCDEFGHIJKLMNOPQRSTUVWXY-abcdefghijklmnopqrstuvwx"
        self.state.control({"op": "plc_values", "values": {"sn": full}})
        self.assertEqual(self.read(4, 16), "3232" + full.encode().hex())
        self.state.control({"op": "plc_values", "values": {"sn": "SN-1"}})
        self.assertEqual(self.read(4, 16), "3204" + b"SN-1".hex() + "00" * 46)
        self.assertEqual(self.state.field_values()["sn"], "SN-1")

    def test_invalid_value_rejects_whole_batch_and_keeps_memory(self):
        cases = [{"sn": value} for value in ("", "A" * 51, "   ", "SN\t1", "SN-é", 12345)] + [{"sn": "SN-1", "productCode": 32768}]
        for values in cases:
            with self.subTest(values=values):
                with self.assertRaises(ValueError):
                    self.state.control({"op": "plc_values", "values": {"requestSeq": 5, **values, "plcState": 10}})
                self.assertEqual(self.read(4, 16), self.EMPTY)
                self.assertEqual(self.state.field_values()["requestSeq"], 0)
                self.assertEqual(self.state.field_values()["plcState"], 0)


class ProductionTransitionTests(unittest.TestCase):
    def setUp(self):
        self.state = PLC.PlcState()
        self.state.heartbeat_enabled = False

    def apply(self, op, **command):
        return self.state.control({"op": op, **command})

    def plc_view(self):
        fields = self.state.field_values()
        return fields["plcState"], fields["plcReason"]

    def test_request_needs_idle_plc_and_ready_pc_and_increments_sequence(self):
        with self.assertRaises(ValueError):
            self.apply("plc_request", values={"productCode": 7, "sn": "SN-1"})
        self.assertEqual((self.plc_view(), self.state.field_values()["requestSeq"]), ((0, 0), 0))
        self.apply("plc_values", values={"pcState": 10})
        self.assertEqual(self.apply("plc_request", values={"productCode": 7, "sn": "SN-1"})["requestSeq"], 1)
        self.apply("plc_values", values={"plcState": 0})
        second = self.apply("plc_request", values={"productCode": 7, "sn": "SN-2"})
        self.assertEqual((second["requestSeq"], second["plcState"], second["sn"]), (2, 10, "SN-2"))
        self.apply("plc_values", values={"requestSeq": PLC.MAX_SEQ, "plcState": 0})
        exhausted = self.apply("plc_request", values={"productCode": 7, "sn": "SN-3"})
        self.assertEqual((exhausted["plcState"], exhausted["plcReason"], exhausted["requestSeq"]), (90, 12, PLC.MAX_SEQ))

    def test_ack_checks_echo_and_result_combination(self):
        cases = [
            (20, {}, (30, 0)),
            (20, {"snEcho": "sn-12345"}, (90, 5)),
            (20, {"snEcho": "SN-12345 "}, (90, 5)),
            (20, {"snEcho": "SN-1234"}, (90, 5)),
            (20, {"seqEcho": 4}, (90, 5)),
            (20, {"resultCode": 0}, (90, 5)),
            (20, {"resultCode": 0, "pcMode": 1}, (30, 0)),
            (20, {"resultCode": 11, "faultCode": 92}, (90, 5)),
            (20, {"resultCode": 90, "faultCode": 0}, (90, 5)),
            (10, {"resultCode": 90, "faultCode": 95}, (30, 0)),
            (10, {}, (90, 5)),
        ]
        for plc_state, overrides, expected in cases:
            with self.subTest(plc_state=plc_state, overrides=overrides):
                self.apply("plc_values", values={"requestSeq": 5, "sn": "SN-12345", "plcReason": 0, "plcState": plc_state})
                self.apply("plc_values", values={"pcMode": 0, "seqEcho": 5, "snEcho": "SN-12345", "resultCode": 1, "faultCode": 0, **overrides, "pcState": 30})
                self.apply("plc_ack")
                self.assertEqual(self.plc_view(), expected)

    def test_reset_paths_follow_transition_table(self):
        cases = [
            (10, 0, 99, "plc_reset", {}, (0, 0)),
            (20, 0, 99, "plc_reset", {}, (0, 0)),
            (30, 0, 99, "plc_reset", {}, (0, 0)),
            (90, 8, 99, "plc_reset", {}, (0, 0)),
            (90, 8, 99, "plc_reset", {"allowed": False}, (90, 13)),
            (90, 8, 0, "plc_reset_request", {}, (99, 0)),
            (99, 0, 10, "plc_reset", {}, (0, 0)),
            (99, 0, 99, "plc_reset", {}, (0, 0)),
            (90, 8, 10, "plc_reset", {}, None),
            (0, 0, 99, "plc_reset", {}, None),
        ]
        for plc_state, reason, pc_state, op, options, expected in cases:
            with self.subTest(plc_state=plc_state, pc_state=pc_state, op=op, options=options):
                self.apply("plc_values", values={"plcReason": reason, "plcState": plc_state, "pcState": pc_state})
                if expected is None:
                    with self.assertRaises(ValueError):
                        self.apply(op, **options)
                    self.assertEqual(self.plc_view(), (plc_state, reason))
                else:
                    self.apply(op, **options)
                    self.assertEqual(self.plc_view(), expected)


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

    def test_normal_cycle_advances_only_on_pc_states_written_over_s7(self):
        def pc_write(connection, byte, data):
            self.assertEqual(exchange(connection, write_bytes_packet(3, 2, byte, data)), WRITE_ACK)

        with connect((self.fixture.ready["host"], self.fixture.ready["port"])) as connection:
            pc_write(connection, 0, words(7, 1, 10, 0))
            request = self.fixture.command("plc_request", values={"productCode": 7, "sn": "SN-12345"})["result"]
            self.assertEqual((request["plcState"], request["requestSeq"], request["sn"]), (10, 1, "SN-12345"))
            self.assertFalse(self.fixture.command("plc_motion_end")["ok"])
            pc_write(connection, 8, (1).to_bytes(4, "big"))
            pc_write(connection, 4, words(20))
            self.assertEqual(self.fixture.command("plc_motion_end")["result"]["plcState"], 20)
            pc_write(connection, 12, words(1, 0) + s7_string("SN-12345"))
            pc_write(connection, 4, words(30))
            self.assertEqual(self.fixture.command("plc_ack")["result"]["plcState"], 30)
            self.assertFalse(self.fixture.command("plc_release")["ok"])
            pc_write(connection, 4, words(10))
        released = self.fixture.command("plc_release")["result"]
        self.assertEqual((released["plcState"], released["plcReason"]), (0, 0))
        self.assertEqual(self.fixture.command("plc_request", values={"productCode": 7, "sn": "SN-2"})["result"]["requestSeq"], 2)

    def test_nondefault_db_is_readable_through_real_s7(self):
        fixture = ProcessFixture("--plc-db", "321", "--pc-db", "322")
        try:
            self.assertEqual((fixture.ready["plc_db"], fixture.ready["pc_db"]), (321, 322))
            fixture.command("plc_values", values={"requestSeq": 7})
            with connect((fixture.ready["host"], fixture.ready["port"])) as connection:
                request = READ_BYTES.replace(bytes.fromhex("000484000040"), bytes.fromhex("014184000040"))
                self.assertEqual(exchange(connection, request)[25:29], b"\x00\x00\x00\x07")
        finally:
            fixture.close()


if __name__ == "__main__":
    unittest.main()
