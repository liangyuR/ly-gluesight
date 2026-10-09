"""Loopback Modbus TCP PLC. Robot and GlueSight use independent TCP connections."""
import argparse
import datetime
import json
import socketserver
import struct
import threading
from pathlib import Path

from modbus_wire import COILS, receive
from simulator_config import DEFAULT_CONFIG, load_config


class Memory:
    def __init__(self, output=None):
        self.lock = threading.RLock()
        self.coils = [False] * 256
        self.registers = [0] * 256
        self.logfile = Path(output) / "plc-wire.jsonl" if output else None
        if self.logfile:
            self.logfile.parent.mkdir(parents=True, exist_ok=True)

    def event(self, **data):
        if self.logfile:
            data["ts"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            with self.logfile.open("a", encoding="utf-8") as out:
                out.write(json.dumps(data, ensure_ascii=False) + "\n")

    def dispatch(self, pdu):
        with self.lock:
            return self._dispatch(pdu)

    def _dispatch(self, pdu):
        fn = pdu[0]
        if fn not in (1, 2, 3, 4, 5, 6, 15, 16):
            return bytes([fn | 128, 1])
        if len(pdu) < 5:
            return bytes([fn | 128, 3])
        start, value = struct.unpack(">HH", pdu[1:5])
        bits = fn in (1, 2, 5, 15)
        bank = self.coils if bits else self.registers
        count = 1 if fn in (5, 6) else value
        maximum = {1:2000, 2:2000, 3:125, 4:125, 5:1, 6:1, 15:1968, 16:123}[fn]
        if not 1 <= count <= maximum:
            return bytes([fn | 128, 3])
        if start + count > len(bank):
            return bytes([fn | 128, 2])
        if fn in (1, 2, 3, 4):
            if len(pdu) != 5:
                return bytes([fn | 128, 3])
            values = bank[start:start + count]
            if bits:
                data = bytearray((count + 7) // 8)
                for i, item in enumerate(values):
                    data[i // 8] |= int(item) << (i % 8)
            else:
                data = struct.pack(">" + "H" * count, *values)
            return bytes([fn, len(data)]) + data
        if fn == 5:
            if len(pdu) != 5 or value not in (0, 0xFF00):
                return bytes([fn | 128, 3])
            values = [bool(value)]
        elif fn == 6:
            if len(pdu) != 5:
                return bytes([fn | 128, 3])
            values = [value]
        else:
            size = (count + 7) // 8 if bits else count * 2
            if len(pdu) != 6 + size or pdu[5] != size:
                return bytes([fn | 128, 3])
            values = ([bool(pdu[6 + i // 8] & (1 << (i % 8))) for i in range(count)]
                      if bits else list(struct.unpack(">" + "H" * count, pdu[6:])))
        for i, item in enumerate(values, start):
            if bank[i] != item:
                self.event(event="write", address=("C" if bits else "HR") + str(i),
                           name=COILS.get(i, "") if bits else "", old=bank[i], value=item)
                bank[i] = item
        return pdu[:5]


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        # Idle connections may span long robot movements. Only a partially received
        # packet has a deadline, so slow motion does not disconnect a valid client.
        try:
            while True:
                first = receive(self.request, 1)
                self.request.settimeout(self.server.frame_timeout)
                tid, protocol, length, unit = struct.unpack(">HHHB", first + receive(self.request, 6))
                if protocol != 0 or not 2 <= length <= 254:
                    return
                pdu = receive(self.request, length - 1)
                reply = (self.server.memory.dispatch(pdu) if unit == self.server.unit
                         else bytes([pdu[0] | 128, 11]))
                self.request.sendall(struct.pack(">HHHB", tid, 0, len(reply) + 1, unit) + reply)
                self.request.settimeout(None)
        except (EOFError, OSError):
            pass


class PlcServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, port=1502, unit=1, output=None, frame_timeout=10):
        self.unit = unit
        self.frame_timeout = frame_timeout
        self.memory = Memory(output)
        super().__init__(("127.0.0.1", port), Handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--port", type=int)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    cfg = load_config(args.config)
    port = args.port if args.port is not None else cfg["plc"]["port"]
    with PlcServer(port, cfg["plc"]["unitId"], args.output or cfg["runtimeDir"]) as server:
        server.memory.event(event="listening", host="127.0.0.1", port=server.server_address[1], unit=server.unit)
        print(f"PLC Modbus TCP ready on 127.0.0.1:{server.server_address[1]}, unit {server.unit}", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
