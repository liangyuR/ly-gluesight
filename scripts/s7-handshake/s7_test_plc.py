from __future__ import annotations

import argparse
from collections import deque
import json
import os
import socket
import socketserver
import struct
import sys
import threading
import time
from dataclasses import dataclass


def integer(value, name, minimum=0, maximum=65535):
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise ValueError(f"{name} must be an integer in {minimum}..{maximum}")
    return value


class ItemError(Exception):
    def __init__(self, code):
        self.code = code


@dataclass(frozen=True)
class Item:
    transport: int
    count: int
    db: int
    area: int
    address: int

    @property
    def byte(self):
        return self.address // 8

    @property
    def bit(self):
        return self.address % 8

    def validate(self):
        if self.area != 0x84 or self.transport not in (1, 2):
            raise ItemError(0x06)
        if not self.count or (self.transport == 1 and self.count != 1):
            raise ItemError(0x07)
        if self.transport == 2 and self.bit:
            raise ItemError(0x05)


def handshake_fields(db):
    fields = {}
    for byte, names in ((0, ("partStart", "partEnd", "resultAck", "faultReset", "plcHeartbeat")), (64, ("visionReady", "armed", "busy", "done", "visionFault", "pcHeartbeat"))):
        for bit, name in enumerate(names):
            fields[name] = {"db": db, "byte": byte, "bit": bit, "type": "bool"}
    words = {"protocolVersion": 2, "productCode": 12, "shotCount": 14, "camera1Shots": 24, "camera2Shots": 26, "camera3Shots": 28, "camera1Triggers": 36, "camera2Triggers": 38, "camera3Triggers": 40, "pcProtocolVersion": 66, "resultCode": 76, "faultCode": 78}
    dwords = {"requestSeq": 4, "partSn": 8, "planVersion": 16, "planReserved": 20, "ackSeq": 32, "resultSeq": 68, "resultSn": 72, "acceptedSeq": 80, "acceptedPlanReserved": 84}
    for kind, addresses in (("u16", words), ("u32", dwords)):
        for name, byte in addresses.items():
            fields[name] = {"db": db, "byte": byte, "type": kind}
    return fields


class PlcState:
    def __init__(self, db_size=1024, pdu_size=960, db=100):
        self.lock = threading.RLock()
        self.stopping = threading.Event()
        self.default_db = db
        self.dbs = {db: bytearray(db_size)}
        self.pdu_size = pdu_size
        self.faults = deque()
        self.events = deque(maxlen=512)
        self.fields = {}
        self.counts = dict(connections=0, active_connections=0, requests=0, reads=0, writes=0, faults=0)
        self.heartbeat_enabled = True
        self.heartbeat_ms = 500
        self.configure_fields(handshake_fields(db))
        self.plc_values({"protocolVersion": 1}, None)

    def event(self, event_kind, **details):
        with self.lock:
            self.events.append({"event": event_kind, "time_ns": time.monotonic_ns(), **details})

    def heartbeat(self):
        while not self.stopping.wait(self.heartbeat_ms / 1000):
            with self.lock:
                if self.heartbeat_enabled and "plcHeartbeat" in self.fields:
                    current = self.field_values()["plcHeartbeat"]
                    self.plc_values({"plcHeartbeat": not current}, None)

    def memory(self, db, byte, size):
        if db not in self.dbs:
            raise ItemError(0x0A)
        buf = self.dbs[db]
        if byte < 0 or size < 0 or byte + size > len(buf):
            raise ItemError(0x05)
        return buf

    def read_item(self, item):
        item.validate()
        with self.lock:
            buf = self.memory(item.db, item.byte, 1 if item.transport == 1 else item.count)
            if item.transport == 1:
                return 3, 1, bytes([(buf[item.byte] >> item.bit) & 1])
            return 4, item.count * 8, bytes(buf[item.byte:item.byte + item.count])

    def write_item(self, item, transport, bits, data):
        item.validate()
        if item.transport == 1:
            if transport != 3:
                raise ItemError(0x06)
            if bits != 1 or len(data) != 1 or data[0] not in (0, 1):
                raise ItemError(0x07)
        elif transport != 4:
            raise ItemError(0x06)
        elif bits != item.count * 8 or len(data) != item.count:
            raise ItemError(0x07)
        with self.lock:
            buf = self.memory(item.db, item.byte, len(data))
            if item.transport == 1:
                mask = 1 << item.bit
                buf[item.byte] = (buf[item.byte] & ~mask) | (data[0] << item.bit)
            else:
                buf[item.byte:item.byte + len(data)] = data

    @staticmethod
    def matches(fault, item):
        return ("db" not in fault or fault["db"] == item.db) and (
            "byte" not in fault or item.byte <= fault["byte"] < item.byte + (1 if item.transport == 1 else item.count)
        ) and ("bit" not in fault or (item.transport == 1 and fault["bit"] == item.bit))

    def take_fault(self, function, items):
        with self.lock:
            for index, fault in enumerate(self.faults):
                if fault["function"] not in (function, "any"):
                    continue
                if any(k in fault for k in ("db", "byte", "bit")) and not any(self.matches(fault, item) for item in items):
                    continue
                chosen = dict(fault)
                fault["count"] -= 1
                if not fault["count"]:
                    del self.faults[index]
                self.counts["faults"] += 1
                return chosen
        return None

    def add_fault(self, command):
        function = command.get("function", "any")
        kind = command.get("kind")
        if kind in ("heartbeat_stop", "heartbeat_resume"):
            with self.lock:
                self.heartbeat_enabled = kind == "heartbeat_resume"
            return {"heartbeat_enabled": self.heartbeat_enabled}
        if function not in ("read", "write", "setup", "any"):
            raise ValueError("function must be read, write, setup or any")
        if kind not in ("reject", "delay", "disconnect"):
            raise ValueError("kind must be reject, delay or disconnect")
        fault = {"function": function, "kind": kind, "count": integer(command.get("count", 1), "count", 1, 10000)}
        for key, lower, upper in (("db", 1, 65535), ("byte", 0, 65535), ("bit", 0, 7), ("code", 1, 254)):
            if key in command:
                fault[key] = integer(command[key], key, lower, upper)
        if kind == "delay":
            fault["delay_ms"] = integer(command.get("delay_ms", 100), "delay_ms", 1, 60000)
        if "after_apply" in command:
            if not isinstance(command["after_apply"], bool) or kind != "disconnect":
                raise ValueError("after_apply is a boolean for disconnect faults")
            fault["after_apply"] = command["after_apply"]
        with self.lock:
            self.faults.append(fault)
        return dict(fault)

    def configure_fields(self, fields):
        if not isinstance(fields, dict) or not fields:
            raise ValueError("fields must be a non-empty object")
        parsed = {}
        for name, source in fields.items():
            if not isinstance(name, str) or not name or not isinstance(source, dict):
                raise ValueError("each field requires a name and address object")
            field = {"db": integer(source.get("db", self.default_db), "db", 1), "byte": integer(source.get("byte"), "byte")}
            field["type"] = source.get("type", "bool" if "bit" in source else "u8")
            if field["type"] not in ("bool", "u8", "u16", "u32"):
                raise ValueError("field type must be bool, u8, u16 or u32")
            if field["type"] == "bool":
                field["bit"] = integer(source.get("bit"), "bit", 0, 7)
            size = {"bool": 1, "u8": 1, "u16": 2, "u32": 4}[field["type"]]
            with self.lock:
                self.memory(field["db"], field["byte"], size)
            parsed[name] = field
        with self.lock:
            self.fields = parsed
        return parsed

    def field_values(self):
        with self.lock:
            result = {}
            for name, field in self.fields.items():
                buf = self.dbs[field["db"]]
                byte = field["byte"]
                if field["type"] == "bool":
                    result[name] = bool((buf[byte] >> field["bit"]) & 1)
                else:
                    size = {"u8": 1, "u16": 2, "u32": 4}[field["type"]]
                    result[name] = int.from_bytes(buf[byte:byte + size], "big")
            return result

    def plc_values(self, values, default_signal):
        if not isinstance(values, dict):
            raise ValueError("values must be an object")
        with self.lock:
            updates = dict(values)
            if default_signal in self.fields:
                updates.setdefault(default_signal, True)
            if not updates:
                raise ValueError("configure handshake fields and provide values first")
            encoded = []
            for name, value in updates.items():
                if name not in self.fields:
                    raise ValueError(f"unknown handshake field: {name}")
                field = self.fields[name]
                if field["type"] == "bool":
                    if not isinstance(value, bool):
                        raise ValueError(f"{name} must be a boolean")
                    item = Item(1, 1, field["db"], 0x84, field["byte"] * 8 + field["bit"])
                    encoded.append((item, 3, 1, bytes([value])))
                else:
                    size = {"u8": 1, "u16": 2, "u32": 4}[field["type"]]
                    value = integer(value, name, 0, (1 << (size * 8)) - 1)
                    encoded.append((Item(2, size, field["db"], 0x84, field["byte"] * 8), 4, size * 8, value.to_bytes(size, "big")))
            for update in encoded:
                self.write_item(*update)
            self.event("plc_values", values=updates)
            return self.field_values()

    def control(self, command):
        if not isinstance(command, dict):
            raise ValueError("command must be an object")
        op = command.get("op")
        if op in ("read_db", "write_db", "set_bit"):
            db = integer(command.get("db", self.default_db), "db", 1)
            byte = integer(command.get("byte", 0), "byte")
            with self.lock:
                if op == "read_db":
                    size = integer(command.get("size", 1), "size", 1)
                    buf = self.memory(db, byte, size)
                    data = bytes(buf[byte:byte + size])
                    return {"db": db, "byte": byte, "hex": data.hex(), "data": list(data)}
                if op == "set_bit":
                    bit = integer(command.get("bit"), "bit", 0, 7)
                    value = command.get("value")
                    if not isinstance(value, bool):
                        raise ValueError("value must be a boolean")
                    self.write_item(Item(1, 1, db, 0x84, byte * 8 + bit), 3, 1, bytes([value]))
                else:
                    if ("hex" in command) == ("data" in command):
                        raise ValueError("provide exactly one of hex or data")
                    if "hex" in command:
                        if not isinstance(command["hex"], str):
                            raise ValueError("hex must be a string")
                        data = bytes.fromhex(command["hex"])
                    else:
                        if not isinstance(command["data"], list):
                            raise ValueError("data must be an array")
                        data = bytes(integer(value, "data byte", 0, 255) for value in command["data"])
                    if not data:
                        raise ValueError("data must not be empty")
                    buf = self.memory(db, byte, len(data))
                    buf[byte:byte + len(data)] = data
                return {"db": db, "byte": byte, "written": True}
        if op == "create_db":
            db = integer(command.get("db"), "db", 1)
            size = integer(command.get("size", 1024), "size", 1, 65536)
            with self.lock:
                if db in self.dbs:
                    raise ValueError("DB already exists")
                if len(self.dbs) >= 16:
                    raise ValueError("at most 16 DBs are supported")
                self.dbs[db] = bytearray(size)
            return {"db": db, "size": size}
        if op == "fault":
            return self.add_fault(command)
        if op == "clear_faults":
            with self.lock:
                self.faults.clear()
            return {"cleared": True}
        if op == "configure_handshake":
            return self.configure_fields(command.get("fields"))
        if op in ("plc_request", "plc_ack", "plc_values"):
            values = command.get("values", {})
            if op == "plc_ack" and isinstance(values, dict):
                with self.lock:
                    current = self.field_values()
                    values = dict(values)
                    if "ackSeq" in current and "resultSeq" in current:
                        values.setdefault("ackSeq", current["resultSeq"])
            return self.plc_values(values, {"plc_request": "partStart", "plc_ack": "resultAck"}.get(op))
        if op == "plc_release":
            with self.lock:
                current = self.field_values()
                if current.get("done", False) or current.get("busy", False):
                    raise ValueError("wait for PC done and busy to clear before PLC release")
                return self.plc_values({name: False for name in ("partStart", "partEnd", "resultAck") if name in current}, None)
        if op == "heartbeat":
            enabled = command.get("enabled", self.heartbeat_enabled)
            if not isinstance(enabled, bool):
                raise ValueError("enabled must be a boolean")
            interval = integer(command.get("interval_ms", self.heartbeat_ms), "interval_ms", 10, 60000)
            with self.lock:
                self.heartbeat_enabled = enabled
                self.heartbeat_ms = interval
            return {"enabled": enabled, "interval_ms": interval}
        if op == "status":
            with self.lock:
                return {"counts": dict(self.counts), "faults": list(self.faults), "fields": self.field_values(), "heartbeat": {"enabled": self.heartbeat_enabled, "interval_ms": self.heartbeat_ms}, "dbs": {str(k): len(v) for k, v in self.dbs.items()}}
        if op == "trace":
            with self.lock:
                events = list(self.events)
                if command.get("clear", False):
                    self.events.clear()
            return events
        if op == "shutdown":
            return {"stopping": True}
        raise ValueError(f"unsupported op: {op}")


def tpkt(payload):
    return struct.pack(">BBH", 3, 0, len(payload) + 4) + payload


def ack(reference, parameters, data=b"", error=b"\x00\x00"):
    header = struct.pack(">BBHHHH", 0x32, 3, 0, reference, len(parameters), len(data)) + error
    return tpkt(b"\x02\xf0\x80" + header + parameters + data)


def parse_items(parameters):
    if len(parameters) < 2:
        raise ValueError("missing item count")
    count = parameters[1]
    if not 1 <= count <= 20 or len(parameters) != 2 + 12 * count:
        raise ValueError("invalid item list length")
    items = []
    for start in range(2, len(parameters), 12):
        spec = parameters[start:start + 12]
        if spec[:3] != b"\x12\x0a\x10":
            raise ValueError("unsupported variable specification")
        items.append(Item(spec[3], int.from_bytes(spec[4:6], "big"), int.from_bytes(spec[6:8], "big"), spec[8], int.from_bytes(spec[9:12], "big")))
    return items


def parse_write_data(items, data):
    result = []
    cursor = 0
    for index, item in enumerate(items):
        if cursor + 4 > len(data):
            raise ValueError("missing write data header")
        reserved, transport, bits = struct.unpack_from(">BBH", data, cursor)
        length = (bits + 7) // 8
        cursor += 4
        if reserved != 0 or cursor + length > len(data):
            raise ValueError("invalid write data length")
        result.append((item, transport, bits, data[cursor:cursor + length]))
        cursor += length
        if length % 2 and index + 1 < len(items):
            if cursor >= len(data) or data[cursor] != 0:
                raise ValueError("missing inter-item padding")
            cursor += 1
    if cursor != len(data):
        raise ValueError("trailing write data")
    return result


class S7Handler(socketserver.BaseRequestHandler):
    def receive(self, count, idle=False):
        data = bytearray()
        deadline = None if idle else time.monotonic() + 5
        while len(data) < count and not self.server.state.stopping.is_set():
            try:
                chunk = self.request.recv(count - len(data))
            except socket.timeout:
                if deadline is not None and time.monotonic() >= deadline:
                    raise ValueError("partial packet timeout")
                continue
            if not chunk:
                return None
            if deadline is None:
                deadline = time.monotonic() + 5
            data.extend(chunk)
        return bytes(data) if len(data) == count else None

    def handle(self):
        state = self.server.state
        self.request.settimeout(0.1)
        with state.lock:
            state.counts["connections"] += 1
            state.counts["active_connections"] += 1
        connected = False
        negotiated = False
        pdu_size = state.pdu_size
        try:
            while not state.stopping.is_set():
                head = self.receive(4, idle=True)
                if head is None:
                    return
                if head[:2] != b"\x03\x00":
                    raise ValueError("invalid TPKT version or reserved byte")
                length = int.from_bytes(head[2:4], "big")
                if not 7 <= length <= 65535:
                    raise ValueError("invalid TPKT length")
                body = self.receive(length - 4)
                if body is None:
                    return
                packet = head + body
                if not connected:
                    response = self.connect_response(body)
                    connected = True
                    function = "connect"
                else:
                    if body[:3] != b"\x02\xf0\x80":
                        raise ValueError("expected unsegmented COTP data")
                    response, function, new_pdu = self.exchange(body[3:], negotiated, pdu_size)
                    if new_pdu is not None:
                        pdu_size = new_pdu
                        negotiated = True
                state.event("packet", function=function, request=packet.hex(), response=response.hex() if response is not None else None)
                if response is None:
                    return
                self.request.sendall(response)
        except (OSError, ValueError) as error:
            state.event("connection_error", error=str(error))
        finally:
            with state.lock:
                state.counts["active_connections"] -= 1

    @staticmethod
    def connect_response(body):
        if len(body) < 7 or body[0] != len(body) - 1 or body[1] != 0xE0 or body[6] != 0:
            raise ValueError("invalid COTP connection request")
        parameters = {}
        cursor = 7
        while cursor < len(body):
            if cursor + 2 > len(body):
                raise ValueError("invalid COTP parameter")
            code, size = body[cursor:cursor + 2]
            cursor += 2
            if cursor + size > len(body) or code in parameters:
                raise ValueError("invalid COTP parameter length")
            parameters[code] = body[cursor:cursor + size]
            cursor += size
        if len(parameters.get(0xC1, b"")) != 2 or len(parameters.get(0xC2, b"")) != 2:
            raise ValueError("missing COTP TSAPs")
        tail = b"\xc0\x01\x0a\xc1\x02" + parameters[0xC2] + b"\xc2\x02" + parameters[0xC1]
        return tpkt(bytes([6 + len(tail), 0xD0]) + body[4:6] + b"\x00\x01\x00" + tail)

    def exchange(self, s7, negotiated, pdu_size):
        if len(s7) < 10 or s7[:2] != b"\x32\x01":
            raise ValueError("expected S7 Job header")
        _, _, reserved, reference, parameter_size, data_size = struct.unpack_from(">BBHHHH", s7)
        if reserved != 0 or len(s7) != 10 + parameter_size + data_size or not parameter_size:
            raise ValueError("invalid S7 header lengths")
        state = self.server.state
        with state.lock:
            state.counts["requests"] += 1
        parameters = s7[10:10 + parameter_size]
        data = s7[10 + parameter_size:]
        function = {0xF0: "setup", 4: "read", 5: "write"}.get(parameters[0], "unsupported")
        if function == "setup":
            if parameters[:2] != b"\xf0\x00" or len(parameters) != 8 or data:
                return ack(reference, b"", error=b"\x81\x04"), function, None
            proposed = int.from_bytes(parameters[6:8], "big")
            if proposed < 240:
                return ack(reference, b"", error=b"\x81\x04"), function, None
            fault = state.take_fault(function, [])
            response, disconnected = self.apply_fault(fault, reference)
            if disconnected or response is not None:
                return response, function, None
            chosen = min(proposed, state.pdu_size)
            return ack(reference, b"\xf0\x00\x00\x01\x00\x01" + chosen.to_bytes(2, "big")), function, chosen
        if not negotiated or len(s7) > pdu_size or function == "unsupported":
            return ack(reference, b"", error=b"\x81\x04"), function, None
        try:
            items = parse_items(parameters)
            updates = parse_write_data(items, data) if function == "write" else []
            if function == "read" and data:
                raise ValueError("ReadVar must not contain data")
        except ValueError:
            return ack(reference, b"", error=b"\x81\x04"), function, None
        if function == "read":
            estimated = 14 + sum(4 + (1 if item.transport == 1 else item.count) + ((1 if item.transport == 1 else item.count) % 2 if index + 1 < len(items) else 0) for index, item in enumerate(items))
            if estimated > pdu_size:
                return ack(reference, b"", error=b"\x85\x00"), function, None
        with state.lock:
            state.counts["reads" if function == "read" else "writes"] += 1
        fault = state.take_fault(function, items)
        response, disconnected = self.apply_fault(fault, reference, item_rejection=True)
        if disconnected:
            return response, function, None
        output = bytearray()
        for index, item in enumerate(items):
            try:
                if fault and fault["kind"] == "reject" and state.matches(fault, item):
                    raise ItemError(fault.get("code", 3))
                if function == "read":
                    transport, bits, value = state.read_item(item)
                    output.extend(struct.pack(">BBH", 0xFF, transport, bits) + value)
                    if len(value) % 2 and index + 1 < len(items):
                        output.append(0)
                else:
                    state.write_item(*updates[index])
                    output.append(0xFF)
            except ItemError as error:
                output.extend(bytes([error.code, 0, 0, 0]) if function == "read" else bytes([error.code]))
        response = ack(reference, bytes([parameters[0], len(items)]), bytes(output))
        if len(response) - 7 > pdu_size:
            response = ack(reference, b"", error=b"\x85\x00")
        if fault and fault["kind"] == "disconnect" and fault.get("after_apply", False):
            return None, function, None
        return response, function, None

    def apply_fault(self, fault, reference, item_rejection=False):
        if not fault:
            return None, False
        self.server.state.event("fault", **fault)
        if fault["kind"] == "delay":
            stopped = self.server.state.stopping.wait(fault["delay_ms"] / 1000)
            return None, stopped
        if fault["kind"] == "disconnect" and not fault.get("after_apply", False):
            return None, True
        if fault["kind"] == "reject" and not item_rejection:
            return ack(reference, b"", error=b"\x81\x04"), False
        return None, False


class S7Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = False

    def __init__(self, state):
        self.state = state
        super().__init__(("127.0.0.1", 0), S7Handler)
        self.worker = threading.Thread(target=self.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        self.heartbeat_worker = threading.Thread(target=state.heartbeat, daemon=True)

    def start(self):
        self.worker.start()
        self.heartbeat_worker.start()
        return self

    def stop(self):
        self.state.stopping.set()
        self.shutdown()
        self.server_close()
        self.worker.join(timeout=2)
        self.heartbeat_worker.join(timeout=2)


def main():
    parser = argparse.ArgumentParser(description="Loopback-only S7 ISO-on-TCP test PLC with JSON stdin/stdout control")
    parser.add_argument("--port", type=int, choices=[0], default=0)
    parser.add_argument("--db-size", type=int, default=1024)
    parser.add_argument("--db", type=int, default=100)
    parser.add_argument("--pdu-size", type=int, default=960)
    args = parser.parse_args()
    try:
        integer(args.db_size, "db-size", 88, 65536)
        integer(args.db, "db", 1, 65535)
        integer(args.pdu_size, "pdu-size", 240, 960)
    except ValueError as error:
        parser.error(str(error))
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8")
    state = PlcState(args.db_size, args.pdu_size, args.db)
    server = S7Server(state).start()

    def emit(value):
        print(json.dumps(value, ensure_ascii=False, separators=(",", ":")), flush=True)

    emit({"event": "ready", "protocol": "s7", "host": "127.0.0.1", "port": server.server_address[1], "pid": os.getpid(), "control": "stdio-json", "db": args.db, "db_size": args.db_size})
    try:
        while True:
            line = sys.stdin.readline(1048577)
            if not line:
                break
            request_id = None
            try:
                if len(line) > 1048576:
                    raise ValueError("control request too large")
                command = json.loads(line)
                if isinstance(command, dict):
                    request_id = command.get("id")
                result = state.control(command)
                emit({"id": request_id, "ok": True, "result": result})
                if command.get("op") == "shutdown":
                    break
            except (ValueError, TypeError, ItemError) as error:
                message = f"S7 memory access error 0x{error.code:02x}" if isinstance(error, ItemError) else str(error)
                emit({"id": request_id, "ok": False, "error": message})
    except KeyboardInterrupt:
        pass
    finally:
        server.stop()


if __name__ == "__main__":
    main()
