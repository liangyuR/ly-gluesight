"""Small Modbus TCP wire implementation for the isolated loopback demo."""
import socket
import struct


def receive(sock, size):
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise EOFError("Modbus connection closed")
        data += chunk
    return data


class Client:
    def __init__(self, port=1502, unit=1, timeout=2):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        self.tid = 0
        self.unit = unit

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def close(self):
        self.sock.close()

    def request(self, pdu):
        self.tid = (self.tid + 1) % 65536
        self.sock.sendall(struct.pack(">HHHB", self.tid, 0, len(pdu) + 1, self.unit) + pdu)
        tid, proto, length, unit = struct.unpack(">HHHB", receive(self.sock, 7))
        if (tid, proto, unit) != (self.tid, 0, self.unit) or not 2 <= length <= 254:
            raise ValueError("Invalid Modbus response header")
        result = receive(self.sock, length - 1)
        if result[0] & 0x80:
            raise RuntimeError(f"Modbus exception {result.hex()}")
        if result[0] != pdu[0]:
            raise ValueError("Mismatched Modbus function")
        return result

    def coils(self, start, count):
        result = self.request(struct.pack(">BHH", 1, start, count))
        if len(result) < 2 or result[1] != (count + 7) // 8 or len(result) != result[1] + 2:
            raise ValueError("Invalid coil byte count")
        return [bool(result[2 + i // 8] & (1 << (i % 8))) for i in range(count)]

    def registers(self, start, count):
        result = self.request(struct.pack(">BHH", 3, start, count))
        if len(result) < 2 or result[1] != count * 2 or len(result) != count * 2 + 2:
            raise ValueError("Invalid register byte count")
        return list(struct.unpack(">" + "H" * count, result[2:]))

    def coil(self, address, value):
        request = struct.pack(">BHH", 5, address, 0xFF00 if value else 0)
        if self.request(request) != request:
            raise ValueError("Invalid coil write acknowledgement")

    def write_registers(self, address, values):
        request = struct.pack(">BHHB", 16, address, len(values), len(values) * 2)
        reply = self.request(request + struct.pack(">" + "H" * len(values), *values))
        if reply != request[:5]:
            raise ValueError("Invalid register write acknowledgement")


COILS = {0:"heartbeat", 10:"partStart", 11:"partEnd", 12:"resultAck", 13:"faultReset",
         20:"visionReady", 21:"armed", 22:"busy", 23:"done"}


def snapshot(client):
    coils = client.coils(0, 24)
    regs = client.registers(100, 14)
    return {**{name:coils[i] for i, name in COILS.items()},
            "partSn": (regs[0] << 16) | regs[1], "productCode":regs[2], "shotCount":regs[3],
            "pathProgress":(regs[4] << 16) | regs[5], "resultCode":regs[10],
            "faultCode":regs[11], "resultSn":(regs[12] << 16) | regs[13]}
