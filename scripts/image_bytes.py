"""读取普通或无损压缩的原图，保留 PNM 头中的 Bayer 信息。"""

from pathlib import Path
import zlib

MAX_IMAGE_BYTES = 128 * 1024 * 1024
ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"


def validate_pnm(raw: bytes) -> None:
    if raw[:2] not in (b"P5", b"P6"):
        raise ValueError("压缩原图必须为 P5/P6 PNM")
    cursor = 2
    fields = []
    for _ in range(3):
        while cursor < len(raw):
            if raw[cursor:cursor + 1].isspace():
                cursor += 1
            elif raw[cursor] == ord("#"):
                end = raw.find(b"\n", cursor)
                if end < 0:
                    raise ValueError("PNM 头不完整")
                cursor = end + 1
            else:
                break
        start = cursor
        while cursor < len(raw) and 48 <= raw[cursor] <= 57:
            cursor += 1
        if not 0 < cursor - start <= 9:
            raise ValueError("PNM 尺寸字段无效")
        fields.append(int(raw[start:cursor]))
    width, height, maximum = fields
    channels = 1 if raw[:2] == b"P5" else 3
    size = width * height * channels
    if width <= 0 or height <= 0 or maximum != 255 or size > MAX_IMAGE_BYTES:
        raise ValueError("PNM 尺寸或位深无效，像素不得超过 128 MiB")
    if not raw[cursor:cursor + 1].isspace() or len(raw) != cursor + 1 + size:
        raise ValueError("PNM 像素长度与尺寸不符")


def decode_image_bytes(encoded: bytes) -> bytes:
    if len(encoded) > MAX_IMAGE_BYTES:
        raise ValueError("原图超过 128 MiB")
    compressed = False
    if encoded.startswith(b"\x1f\x8b"):
        compressed = True
        decoder = zlib.decompressobj(wbits=31)
        try:
            raw = decoder.decompress(encoded, MAX_IMAGE_BYTES + 1)
        except zlib.error as error:
            raise ValueError(f"gzip 原图损坏：{error}") from error
        if len(raw) > MAX_IMAGE_BYTES or not decoder.eof or decoder.unused_data:
            raise ValueError("gzip 原图超限、不完整或包含尾随数据")
    elif encoded.startswith(ZSTD_MAGIC):
        compressed = True
        try:
            import zstandard
        except ImportError as error:
            raise ValueError("读取 zstd 原图需要安装 zstandard（见 scripts/requirements-images.txt）") from error
        try:
            parameters = zstandard.get_frame_parameters(encoded)
            if not parameters.has_checksum or parameters.window_size > MAX_IMAGE_BYTES:
                raise ValueError("zstd 原图缺少校验和或窗口超过 128 MiB")
            if parameters.content_size not in (zstandard.CONTENTSIZE_UNKNOWN, zstandard.CONTENTSIZE_ERROR) and parameters.content_size > MAX_IMAGE_BYTES:
                raise ValueError("zstd 原图解压后超过 128 MiB")
            raw = zstandard.ZstdDecompressor(max_window_size=MAX_IMAGE_BYTES // 1024).decompress(
                encoded, max_output_size=MAX_IMAGE_BYTES, allow_extra_data=False,
            )
        except zstandard.ZstdError as error:
            raise ValueError(f"zstd 原图损坏：{error}") from error
    else:
        raw = encoded
    if compressed or raw[:2] in (b"P5", b"P6"):
        validate_pnm(raw)
    return raw


def read_image_bytes(path: Path) -> bytes:
    with path.open("rb") as stream:
        encoded = stream.read(MAX_IMAGE_BYTES + 1)
    return decode_image_bytes(encoded)
