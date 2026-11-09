import { expect, it } from "vitest";
import { loadPreview } from "../src/features/workspace/previewCache";
import type { FrameImage } from "../src/features/workspace/types";

it("快速切帧取消排队旧图 → 当前帧可完成且回看仍取对应图像", async () => {
  const image = (id: string): FrameImage => ({ url: id, width: 1, height: 1 });
  let first!: (value: FrameImage) => void;
  let second!: (value: FrameImage) => void;
  const a = loadPreview("queue-a", () => new Promise(resolve => { first = resolve; }));
  const b = loadPreview("queue-b", () => new Promise(resolve => { second = resolve; }));
  const obsolete = new AbortController();
  const old = loadPreview("queue-obsolete", async () => { throw new Error("已取消图像不应开始读取"); }, obsolete.signal);
  const cancelled = expect(old).rejects.toMatchObject({ name: "AbortError" });
  const current = loadPreview("queue-current", async () => image("current"));
  obsolete.abort();
  await Promise.resolve();
  first(image("first")); second(image("second"));
  await cancelled;
  expect(await current).toEqual(image("current"));
  expect(await a).toEqual(image("first"));
  expect(await b).toEqual(image("second"));
  expect(await loadPreview("queue-current", async () => image("wrong"))).toEqual(image("current"));
});
