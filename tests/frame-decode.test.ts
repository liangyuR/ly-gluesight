import { describe, expect, it } from "vitest";
import { decodeFrame, fillRgba } from "../src/lib/frame";
import { decodeThumbStrip } from "../src/features/workspace/workbenchDraft";

function packet(header: number[], body: number[]) {
  const bytes = new Uint8Array(header.length * 4 + body.length);
  const view = new DataView(bytes.buffer);
  header.forEach((value, i) => view.setUint32(i * 4, value, true));
  bytes.set(body, header.length * 4);
  return bytes.buffer;
}

describe("后端图像字节格式", () => {
  it.each([
    { name: "1 通道", channels: 1, body: [10, 200], rgba: [10, 10, 10, 255, 200, 200, 200, 255] },
    { name: "3 通道", channels: 3, body: [255, 0, 0, 0, 128, 255], rgba: [255, 0, 0, 255, 0, 128, 255, 255] },
  ])("整帧预览 20 字节头 + $name像素 → 原图尺寸取头部，按通道铺成 RGBA", ({ channels, body, rgba }) => {
    const frame = decodeFrame(packet([2, 1, 2448, 2048, channels], body));
    expect([frame.fullWidth, frame.fullHeight]).toEqual([2448, 2048]);
    const out = new Uint8ClampedArray(8);
    fillRgba(frame.pixels, out);
    expect(Array.from(out)).toEqual(rgba);
  });

  it("缩略图条 [组数, 宽, 高, 通道数] 头 + 每组状态字节 → 按通道切出各组像素，缺帧为 null", () => {
    const strip = decodeThumbStrip(packet([2, 1, 1, 3], [1, 9, 8, 7, 0, 0, 0, 0]));
    expect(strip).toMatchObject({ width: 1, height: 1, channels: 3 });
    expect(strip.frames.map(frame => frame && Array.from(frame))).toEqual([[9, 8, 7], null]);
  });
});
