import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { expect, it, vi } from "vitest";
import TeachingPage from "../src/features/workspace/TeachingPage";
import { WorkspaceProvider } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi, useCycle } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera";
import { snapshot, summary, tricamWorkspaceView, twoLines } from "./fixtures";

vi.mock("../src/features/workspace/api", () => ({ workspaceApi: {
  get: vi.fn(), list: vi.fn(), image: vi.fn(), progress: vi.fn(),
  saveDraft: vi.fn(), saveParams: vi.fn(), trial: vi.fn(), saveTeach: vi.fn(),
} }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn() }, useCycle: vi.fn() }));
vi.mock("../src/features/camera", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc", () => ({ subscribe: () => () => {} }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: () => true }));

it("从采集原图完成两个拍照点的中线、试测和示教，再进入总览", async () => {
  vi.stubGlobal("DOMPoint", class {
    constructor(public x: number, public y: number) {}
    matrixTransform() { return this; }
  });
  vi.stubGlobal("PointerEvent", MouseEvent);
  Object.defineProperty(SVGElement.prototype, "getScreenCTM", { configurable: true, value: () => ({ inverse: () => ({}) }) });
  Object.defineProperty(SVGElement.prototype, "setPointerCapture", { configurable: true, value: vi.fn() });

  const candidate = tricamWorkspaceView();
  const trials = candidate.workspace.frames.map(frame => structuredClone(frame.trial!));
  candidate.workspace.captureId = "captured-round";
  candidate.workspace.validation = null;
  candidate.coverage = 0;
  candidate.workspace.doc.shots.forEach(shot => { shot.path = []; delete shot.mmPerPx; });
  candidate.workspace.frames.forEach(frame => { frame.trial = null; frame.saved = false; });
  candidate.layout.shots = structuredClone(candidate.workspace.doc.shots);
  candidate.layout.segments = [];
  candidate.layout.points = { x: [], y: [], seg: [], k: [] };

  vi.mocked(workspaceApi.get).mockResolvedValue(candidate);
  vi.mocked(workspaceApi.list).mockResolvedValue([candidate.workspace]);
  vi.mocked(workspaceApi.image).mockResolvedValue({ url: "data:image/png;base64,AA==", width: 100, height: 60 });
  vi.mocked(workspaceApi.progress).mockImplementation(async (_id, k, view) => ({
    ...candidate, workspace: { ...candidate.workspace, lastPosition: { k, view } },
  }));
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: [summary(candidate)], errors: [] });
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
  vi.mocked(useCycle).mockReturnValue({ snapshot: snapshot(), logs: [], measured: [] });

  let response = structuredClone(candidate);
  for (const k of [0, 1]) {
    response.workspace.revision++;
    response.workspace.doc.shots[k].path = structuredClone(twoLines[k]);
    response.workspace.doc.shots[k].mmPerPx = .1;
    response.layout.shots = structuredClone(response.workspace.doc.shots);
    vi.mocked(workspaceApi.saveParams).mockResolvedValueOnce(structuredClone(response));
    response.workspace.frames[k].trial = trials[k];
    vi.mocked(workspaceApi.trial).mockResolvedValueOnce(structuredClone(response));
    response.workspace.frames[k].saved = true;
    response.workspace.revision++;
    vi.mocked(workspaceApi.saveTeach).mockResolvedValueOnce(structuredClone(response));
  }

  render(<MemoryRouter initialEntries={["/recipe/teach"]}><WorkspaceProvider><Routes>
    <Route path="/recipe/teach" element={<TeachingPage />} />
    <Route path="/recipe/overview" element={<h1>工件总览</h1>} />
  </Routes></WorkspaceProvider></MemoryRouter>);
  const user = userEvent.setup();
  expect(await screen.findByText(/全部 0／2 幅/)).toBeVisible();

  for (const k of [0, 1]) {
    const imageId = `A-image-${k}`;
    const image = await screen.findByRole("img", { name: `冻结图像 · ${imageId}` });
    expect(screen.getByRole("button", { name: "选择帧 k" + (k + 1) })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "试测当前帧" })).toBeDisabled();
    for (const [clientX, clientY] of twoLines[k]) {
      fireEvent.pointerDown(image, { clientX, clientY });
      fireEvent.pointerUp(image);
    }
    fireEvent.change(screen.getByRole("spinbutton", { name: "像素当量" }), { target: { value: ".1" } });
    expect(screen.getByLabelText("中线点 2")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "保存中线" }));
    expect(workspaceApi.saveParams).toHaveBeenNthCalledWith(k + 1, "A", 7 + 2 * k, k, { path: twoLines[k], mmPerPx: .1 });
    await waitFor(() => expect(screen.getByRole("button", { name: "试测当前帧" })).toBeEnabled());
    expect(screen.queryByText("示教中线有未保存的修改")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "试测当前帧" }));
    expect(workspaceApi.trial).toHaveBeenNthCalledWith(k + 1, "A", 8 + 2 * k, k, imageId);
    await waitFor(() => expect(screen.getByRole("button", { name: "保存并示教下一幅" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "保存并示教下一幅" }));
    expect(workspaceApi.saveTeach).toHaveBeenNthCalledWith(k + 1, "A", 8 + 2 * k, k, imageId);
    await screen.findByText(new RegExp(`全部 ${k + 1}／2 幅`));
  }

  expect(screen.getByRole("button", { name: "保存本帧示教" })).toBeDisabled();
  await user.click(screen.getByRole("link", { name: "布置总览" }));
  expect(await screen.findByRole("heading", { name: "工件总览" })).toBeVisible();
  expect(workspaceApi.saveParams).toHaveBeenCalledTimes(2);
  expect(workspaceApi.trial).toHaveBeenCalledTimes(2);
  expect(workspaceApi.saveTeach).toHaveBeenCalledTimes(2);
});
