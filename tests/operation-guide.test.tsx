import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import OperationGuidePage from "../src/pages/OperationGuidePage";

vi.mock("../src/features/camera/api", async importOriginal => ({
  ...await importOriginal<typeof import("../src/features/camera/api")>(),
  useRigStatus: () => ({ statuses: [], lastFrame: {} }),
}));
vi.mock("../src/features/plc/api", async importOriginal => ({
  ...await importOriginal<typeof import("../src/features/plc/api")>(),
  usePlcStatus: () => null,
}));
vi.mock("../src/features/workspace/context", () => ({
  useWorkspace: () => ({ list: [], drafts: [] }),
}));

describe("操作流程就绪提示", () => {
  it("没有采集设备时不能显示就绪", () => {
    render(<MemoryRouter><OperationGuidePage /></MemoryRouter>);
    expect(screen.getByText("0/0 就绪")).toHaveClass("warn");
  });
});
