"""读录制原图的原始字节：软件空闲时把录制目录里的 `X.pgm` 无损压缩成同目录同主名的 `X.gsr`，记录里的文件名仍是 `X.pgm`。

格式定义见 `src-tauri/src/image_file/gsr.rs`。读 `.gsr` 需要 zstandard 与 numpy；目录里没有 `.gsr` 时不需要 zstandard。
"""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

MAGIC = b"GSR\x1a"
VERSION = 1
FIXED = 16
MAX_HEADER = 512
MAX_PIXELS = 256 << 20
ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"
SUFFIX = ".gsr"


class GsrError(ValueError):
    pass


def _dimensions(header: bytes) -> tuple[int, int, int]:
    """PNM 文件头的 (宽, 高, 每像素字节数)。"""
    channels = {b"P5": 1, b"P6": 3}.get(header[:2])
    if channels is None:
        raise GsrError("压缩原图的文件头不是 P5 / P6")
    fields, cursor = [], 2
    while len(fields) < 3:
        if cursor >= len(header):
            raise GsrError("压缩原图的文件头不完整")
        if header[cursor : cursor + 1].isspace():
            cursor += 1
        elif header[cursor] == ord("#"):
            end = header.find(b"\n", cursor)
            cursor = len(header) if end < 0 else end
        else:
            end = cursor
            while end < len(header) and header[end : end + 1].isdigit():
                end += 1
            if end == cursor:
                raise GsrError("压缩原图的文件头无效")
            fields.append(int(header[cursor:end]))
            cursor = end
    width, height, maximum = fields
    if maximum != 255 or width <= 0 or height <= 0 or cursor + 1 != len(header):
        raise GsrError("压缩原图的文件头无效")
    return width, height, channels


def decode(data: bytes) -> bytes:
    """`.gsr` 文件内容还原成原始 PNM 文件的全部字节；损坏、截断、带多余数据或校验和不符时抛 GsrError。"""
    import numpy as np
    import zstandard

    if len(data) < FIXED or data[:4] != MAGIC:
        raise GsrError("不是压缩原图文件")
    version, transform, header_len, pixels, crc = struct.unpack_from("<BBHII", data, 4)
    if version != VERSION:
        raise GsrError(f"压缩原图版本 {version} 不受支持")
    if not 0 < header_len <= MAX_HEADER or pixels > MAX_PIXELS:
        raise GsrError("压缩原图的头或像素长度超出上限")
    header = data[FIXED : FIXED + header_len]
    if len(header) != header_len:
        raise GsrError("压缩原图被截断")
    width, height, channels = _dimensions(header)
    if width * height * channels != pixels:
        raise GsrError("压缩原图的文件头与像素长度不一致")
    payload = data[FIXED + header_len :]
    if payload[:4] != ZSTD_MAGIC or len(payload) < 5 or not payload[4] & 0x04:
        raise GsrError("压缩原图的负载不是带校验和的 Zstandard 帧")
    try:
        decompressor = zstandard.ZstdDecompressor().decompressobj()
        raw = decompressor.decompress(payload)
    except zstandard.ZstdError as error:
        raise GsrError(f"压缩原图解压失败：{error}") from error
    if not decompressor.eof or decompressor.unused_data or len(raw) != pixels:
        raise GsrError("压缩原图被截断、带有多余数据或长度与文件头不符")
    values = np.frombuffer(raw, dtype=np.uint8)
    if transform == 1:
        restored = np.cumsum(values.reshape(height, width, channels), axis=1, dtype=np.uint8)
    elif transform == 2:
        if channels != 1 or width % 2 or height % 2:
            raise GsrError("压缩原图的变换无效")
        planes = np.cumsum(values.reshape(4, height // 2, width // 2), axis=2, dtype=np.uint8)
        restored = np.empty((height, width), dtype=np.uint8)
        for phase in range(4):
            restored[phase // 2 :: 2, phase % 2 :: 2] = planes[phase]
    elif transform == 0:
        restored = values
    else:
        raise GsrError(f"压缩原图的变换 {transform} 无效")
    original = header + restored.tobytes()
    if zlib.crc32(original) != crc:
        raise GsrError("压缩原图校验和不符")
    return original


def stored_path(path: Path) -> Path | None:
    """原图实际在盘上的文件：`path` 在就是它；不在而同名的 `.gsr` 在，就是那个 `.gsr`；都不在为 None。"""
    path = Path(path)
    if path.is_file():
        return path
    sibling = path.with_suffix(SUFFIX)
    return sibling if path.suffix.lower() == ".pgm" and sibling.is_file() else None


def read_original(path: Path) -> bytes:
    """原图的原始字节：`path` 是 `.pgm` 而文件已压缩成同名 `.gsr`，或 `path` 本身是 `.gsr` 时还原后返回。"""
    stored = stored_path(path)
    if stored is None:
        raise FileNotFoundError(str(path))
    data = stored.read_bytes()
    return decode(data) if stored.suffix.lower() == SUFFIX else data
