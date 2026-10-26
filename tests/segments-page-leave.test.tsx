import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { MemoryRouter } from "react-router-dom";
import { SnackbarProvider } from "qing-steel";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SegmentsPage from "../src/features/workspace/SegmentsPage";
import { WorkspaceProvider, useWorkspace } from "../src/features/workspace/context";
import { workspaceApi } from "../src/features/workspace/api";
import { recipeApi } from "../src/features/cycle/api";
import { cameraApi } from "../src/features/camera/api";
import { desktopAvailable } from "../src/lib/desktop";
import type { WorkspaceView } from "../src/features/workspace/types";
import { burstRecipe, burstSummary, burstWorkspaceView, deferred } from "./fixtures";
// 段列表页的编辑有 700 ms 防抖：到期前离开页面，编辑要随卸载提交，不能丢。
// 提交走哪条路取决于真实的 WorkspaceProvider（act 抬高请求序号、接受返回的新修订），所以这里不 mock context，
// 断言的是工作区最终的状态，不是 act 被调了几次。页面内的编辑与提交内容见 segments-page.test.tsx。
vi.mock("../src/lib/storage", () => ({ storageApi: { preferences: async () => ({}), updatePreferences: vi.fn(async () => ({})) } }));
vi.mock("../src/lib/desktop", () => ({ desktopAvailable: vi.fn() }));
vi.mock("../src/features/workspace/api", () => ({ workspaceApi: { list: vi.fn(), get: vi.fn(), saveDoc: vi.fn(), updateSegments: vi.fn(), captureList: vi.fn(async () => []) } }));
vi.mock("../src/features/cycle/api", () => ({ recipeApi: { list: vi.fn(), preview: vi.fn() } }));
vi.mock("../src/features/camera/api", () => ({ cameraApi: { rigConfig: vi.fn() } }));
vi.mock("../src/features/plc/api", () => ({ subscribe: vi.fn(() => () => {}) }));

const docs = Object.fromEntries(["A", "B"].map(id => [id, burstRecipe({ id, name: `段配方 ${id}` })]));
const view = (id: string) => burstWorkspaceView(docs[id]);

beforeEach(() => {
  vi.mocked(desktopAvailable).mockReturnValue(true);
  vi.mocked(recipeApi.list).mockResolvedValue({ recipes: Object.values(docs).map(doc => burstSummary(doc)), errors: [], unavailable: [] });
  vi.mocked(recipeApi.preview).mockResolvedValue(view("A").layout);
  vi.mocked(workspaceApi.list).mockResolvedValue([view("A").workspace, view("B").workspace]);
  vi.mocked(workspaceApi.get).mockImplementation(async id => view(id));
  vi.mocked(workspaceApi.updateSegments).mockReset().mockResolvedValue(view("A"));
  vi.mocked(cameraApi.rigConfig).mockResolvedValue([]);
});

/** 页面之外读出工作区当前的配方与修订，并能把页面卸掉（仍留在这个配方）。 */
function Shell() {
  const ws = useWorkspace();
  const [onPage, setOnPage] = useState(true);
  return <>
    <button onClick={() => setOnPage(false)}>离开本页</button>
    <output aria-label="工作区配方">{ws.data?.workspace.doc.id ?? "无"}</output>
    <output aria-label="工作区修订">{ws.data?.workspace.revision ?? "无"}</output>
    {onPage && <SegmentsPage />}
  </>;
}

/** 打开配方 A 的段列表页，并把第 1 段的帧数容差改成 2（防抖没到期，还没提交）。 */
async function editWithoutWaiting() {
  render(<MemoryRouter><SnackbarProvider><WorkspaceProvider><Shell /></WorkspaceProvider></SnackbarProvider></MemoryRouter>);
  const first = within(await screen.findByRole("region", { name: "段 1" }));
  await waitFor(() => expect(screen.getByLabelText("工作区配方")).toHaveTextContent("A"));
  fireEvent.change(first.getByRole("spinbutton", { name: "段 1 帧数容差" }), { target: { value: "2" } });
  expect(workspaceApi.updateSegments).not.toHaveBeenCalled();
}

describe("防抖没到期就离开段列表页", () => {
  // 防的回归：换配方时经 act 提交会让 act 抬高请求序号，打断 select 对新配方的加载，工作区停在空白
  it("换了配方 → 编辑提交到原配方，新配方照常加载出来", async () => {
    const loadingB = deferred<WorkspaceView>();
    vi.mocked(workspaceApi.get).mockImplementation(id => id === "B" ? loadingB.promise : Promise.resolve(view(id)));
    await editWithoutWaiting();

    await userEvent.setup().selectOptions(screen.getByLabelText("当前配方"), "B");
    await act(async () => loadingB.resolve(view("B")));

    expect(workspaceApi.updateSegments).toHaveBeenCalledTimes(1);
    const [id, revision, sent] = vi.mocked(workspaceApi.updateSegments).mock.calls[0];
    expect([id, revision, sent[0].tolerance]).toEqual(["A", 3, 2]);
    await waitFor(() => expect(screen.getByLabelText("工作区配方")).toHaveTextContent("B"));
  });

  // 防的回归：卸载时直接调后端提交，工作区仍握着旧修订，之后的保存被后端按修订冲突拒绝
  it("仍在这个配方 → 提交后工作区拿到返回的新修订", async () => {
    const saved = view("A");
    saved.workspace.revision = 4;
    vi.mocked(workspaceApi.updateSegments).mockResolvedValue(saved);
    await editWithoutWaiting();
    expect(screen.getByLabelText("工作区修订")).toHaveTextContent("3");

    await userEvent.setup().click(screen.getByRole("button", { name: "离开本页" }));

    await waitFor(() => expect(screen.getByLabelText("工作区修订")).toHaveTextContent("4"));
    expect(vi.mocked(workspaceApi.updateSegments).mock.calls[0].slice(0, 2)).toEqual(["A", 3]);
  });
});
