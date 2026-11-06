/** 帧像素：channels 为 1 时是 8 位灰度，为 3 时是 RGB 交错；尺寸可能是降采样后的。 */
export interface FramePixels { data: Uint8Array; width: number; height: number; channels: 1 | 3 }

/** 整帧预览：pixels 为预览尺寸，fullWidth / fullHeight 为原图尺寸。 */
export interface FramePreview { fullWidth: number; fullHeight: number; pixels: FramePixels }

export const FRAME_HEADER_BYTES = 20;

const toBuffer = (input: ArrayBuffer | number[]) => input instanceof ArrayBuffer ? input : Uint8Array.from(input).buffer;

export function frameChannels(value: number): 1 | 3 {
  if (value !== 1 && value !== 3) throw new Error(`不支持的图像通道数：${value}`);
  return value;
}

/** 解析整帧预览：20 字节头 [预览宽, 预览高, 原图宽, 原图高, 通道数]（u32 小端），之后为 预览宽 × 预览高 × 通道数 字节。 */
export function decodeFrame(input: ArrayBuffer | number[]): FramePreview {
  const buf = toBuffer(input);
  if (buf.byteLength < FRAME_HEADER_BYTES) throw new Error("原图预览数据为空");
  const view = new DataView(buf);
  const [width, height, fullWidth, fullHeight, raw] = [0, 4, 8, 12, 16].map(o => view.getUint32(o, true));
  const channels = frameChannels(raw);
  const size = width * height * channels;
  if (!width || !height || width * height > 40_000_000 || buf.byteLength < FRAME_HEADER_BYTES + size) throw new Error("原图预览数据不完整");
  return { fullWidth, fullHeight, pixels: { data: new Uint8Array(buf, FRAME_HEADER_BYTES, size), width, height, channels } };
}

/** 按通道数铺成 RGBA：灰度 r = g = b，彩色逐像素取 RGB。 */
export function fillRgba({ data, width, height, channels }: FramePixels, out: Uint8ClampedArray) {
  const n = width * height;
  if (channels === 1) {
    const words = new Uint32Array(out.buffer, out.byteOffset, n);
    for (let i = 0; i < n; i++) words[i] = 0xff000000 | data[i] * 0x010101;
    return;
  }
  for (let i = 0, j = 0, k = 0; i < n; i++, j += 3, k += 4) {
    out[k] = data[j]; out[k + 1] = data[j + 1]; out[k + 2] = data[j + 2]; out[k + 3] = 255;
  }
}

export function frameImageData(pixels: FramePixels): ImageData {
  const image = new ImageData(pixels.width, pixels.height);
  fillRgba(pixels, image.data);
  return image;
}
