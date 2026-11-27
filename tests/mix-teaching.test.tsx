import { act, fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import MixTeachingCard from "../src/features/workspace/MixTeachingCard";
import { stubSvgPointer } from "./svg-pointer";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));

it("排胶重新拍照 → 清除旧框，纯色端点只保存新冻结帧上确认的像素", async () => {
  stubSvgPointer();
  let sequence = 0;
  const captured = new Set<string>();
  let saved: unknown;
  const frame = new ArrayBuffer(20 + 100 * 60 * 3);
  new Uint32Array(frame, 0, 5).set([100, 60, 100, 60, 3]);
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    switch (command) {
      case "workspace_mix_endpoints": return { yellow: null, blue: null };
      case "workspace_history_candidates": return { total: 0, items: [] };
      case "workspace_mix_sample": {
        const token = `sample-${++sequence}`;
        captured.add(token);
        return { token, greenLines: [] };
      }
      case "workspace_mix_sample_image":
        if (!captured.has((args as {token: string}).token)) throw Error("样本未冻结");
        return frame;
      case "workspace_mix_save_endpoint": saved = args; return { yellow: "new-endpoint", blue: null };
      default: throw Error(`未预期的调用 ${command}`);
    }
  });
  render(<MixTeachingCard id="test" revision={5} camera="cam2" segment={3}
    check={{ backgroundRects: [], maxLengthMm: 2, maxAreaMm2: 3, driftLengthMm: 4 }}
    disabled={false} pending={false} change={vi.fn()} persist={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "示教混胶色差" }));
  const photograph = await screen.findByRole("button", { name: "Software 拍摄排胶样" });
  const drag = (image: Element, x: number, y: number, w: number, h: number) => {
    fireEvent.pointerDown(image, { clientX: x, clientY: y, pointerId: 1 });
    fireEvent.pointerMove(image, { clientX: x + w, clientY: y + h, pointerId: 1 });
    fireEvent.pointerUp(image, { pointerId: 1 });
  };
  const takePhoto = async () => {
    await act(async () => { fireEvent.click(photograph); });
    return screen.getByRole("img", { name: "混胶取样原图" });
  };
  drag(await takePhoto(), 3, 4, 8, 9);
  expect(screen.getByRole("button", { name: "框选为纯黄" })).toBeEnabled();
  const image = await takePhoto();
  expect(screen.getByRole("button", { name: "框选为纯黄" })).toBeDisabled();
  drag(image, 11, 12, 20, 10);
  const save = screen.getByRole("button", { name: "框选为纯黄" });
  expect(save).toBeEnabled();
  await act(async () => { fireEvent.click(save); });
  expect(screen.getByRole("status")).toHaveTextContent("已保存纯黄端点");
  expect(saved).toEqual({ camera: "cam2", token: "sample-2", role: "yellow", rect: [11, 12, 20, 10] });
});
