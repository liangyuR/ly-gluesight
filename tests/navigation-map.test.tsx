import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import NavigationMap from "../src/features/workspace/navigation/NavigationMap";
import type { NavigationMapData } from "../src/features/workspace/navigation/types";

const map: NavigationMapData = { format: 1, image: { id: "image", width: 100, height: 100, mime: "image/png" }, points: [[0, 0.5], [1, 0.5]], closed: false, captureStart: 0, captureEnd: 1, cuts: [0.5], confirmed: true, structureRevision: 0 };
const props = { map, imageUrl: "data:image/png;base64,AA==", segments: [{ index: 1, kind: "straight" as const }, { index: 2, kind: "corner" as const }], ranges: [[1, 4], [5, 8]] as [number, number][], seq: 1, frameStatus: "confirmed" as const, editable: true, onNavigate: vi.fn() };

describe("导航地图编辑与浏览", () => {

  it("未保存草稿重新挂载且服务端地图更新 → 草稿仍在且禁止覆盖新来源", async () => {
    const user = userEvent.setup(), save = vi.fn(), initialDraft = { ...map, cuts: [0.6], confirmed: false };
    const { rerender } = render(<NavigationMap {...props} initialDraft={initialDraft} onSave={save} />);
    expect(screen.getByLabelText("段 1 后分界（路径 %）")).toHaveValue(60);
    rerender(<NavigationMap {...props} map={{ ...map, cuts: [0.7] }} initialDraft={initialDraft} onSave={save} />);
    expect(screen.getByRole("alert")).toHaveTextContent("地图来源已更新");
    expect(screen.getByRole("button", { name: "确认空间对应并保存" })).toBeDisabled();
    expect(screen.getByLabelText("段 1 后分界（路径 %）")).toHaveValue(60);
    await user.click(screen.getByRole("button", { name: "撤销地图修改" }));
    await user.click(screen.getByRole("button", { name: "编辑地图" }));
    await user.click(screen.getByText("精确调整与更多操作", { exact: true }));
    expect(screen.getByLabelText("段 1 后分界（路径 %）")).toHaveValue(70);
  });

  it("旋转翻转并放大后编辑 → 拖点与画点保存原图坐标，方向键按屏幕方向移动", async () => {
    const user = userEvent.setup(), save = vi.fn().mockResolvedValue(true);
    render(<NavigationMap {...props} map={{ ...map, points: [[0.25, 0.5], [0.75, 0.5]] }} onSave={save} />);
    const canvas = screen.getByRole("img", { name: "工件图与有向胶路" });
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({ width: 400, height: 400, x: 0, y: 0, left: 0, top: 0, bottom: 400, right: 400, toJSON() {} });
    Object.defineProperty(canvas, "setPointerCapture", { value: vi.fn() });
    await user.click(screen.getByRole("button", { name: "编辑地图" }));
    await user.click(screen.getByText("精确调整与更多操作", { exact: true }));
    await user.click(screen.getByText("视图设置", { exact: true }));
    await user.click(screen.getByRole("button", { name: "向右旋转90度" }));
    await user.click(screen.getByRole("button", { name: "水平翻转" }));
    await user.click(screen.getByRole("button", { name: "放大地图" }));
    fireEvent.pointerDown(canvas, { clientX: 200, clientY: 350, button: 0 });
    fireEvent.pointerMove(canvas, { clientX: 200, clientY: 290 });
    fireEvent.pointerUp(canvas);
    expect(screen.getByLabelText("顶点 1 X（图片 %）")).toHaveValue(35);
    fireEvent.keyDown(canvas, { key: "ArrowRight" });
    expect(screen.getByLabelText("顶点 1 Y（图片 %）")).toHaveValue(49);
    await user.click(screen.getByRole("button", { name: "1 画胶路" }));
    fireEvent.pointerDown(canvas, { clientX: 200, clientY: 200, button: 0 });
    await user.click(screen.getByRole("button", { name: "保存地图草稿" }));
    const saved = save.mock.calls[0][0] as NavigationMapData;
    expect(saved.points[0][0]).toBeCloseTo(0.35);
    expect(saved.points[0][1]).toBeCloseTo(0.49);
    expect(saved.points[2]).toEqual([0.5, 0.5]);
    expect(saved.cuts).toEqual(map.cuts);
  });

  it("单段地图按步骤标采集范围 → 隐藏无效分界工具，点起点后自动转到终点", async () => {
    const user = userEvent.setup(), save = vi.fn().mockResolvedValue(true);
    render(<NavigationMap {...props} map={{ ...map, cuts: [] }} segments={[props.segments[0]]} ranges={[[1, 8]]} onSave={save} />);
    await user.click(screen.getByRole("button", { name: "编辑地图" }));
    expect(screen.queryByRole("button", { name: "向右旋转90度" })).not.toBeInTheDocument();
    expect(screen.queryByRole("spinbutton", { name: "采集起点（路径 %）" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "3 分段" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "下一步" }));
    expect(screen.getByText("当前仅 1 段，无需标分界。")).toBeVisible();
    const canvas = screen.getByRole("img", { name: "工件图与有向胶路" });
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({ width: 400, height: 400, x: 0, y: 0, left: 0, top: 0, bottom: 400, right: 400, toJSON() {} });
    fireEvent.pointerDown(canvas, { clientX: 100, clientY: 200, button: 0 });
    expect(screen.getByRole("button", { name: "标采集终点" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.pointerDown(canvas, { clientX: 300, clientY: 200, button: 0 });
    await user.click(screen.getByRole("button", { name: "确认空间对应并保存" }));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ captureStart: 0.25, captureEnd: 0.75, cuts: [], points: map.points }), true);
  });

});
