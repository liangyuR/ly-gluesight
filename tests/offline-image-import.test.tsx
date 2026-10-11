import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import ImageImportButton from "../src/features/workspace/ImageImportButton";
import { deferred } from "./fixtures";

function file(name = "offline.pgm", bytes = [80, 53, 10, 0, 255]) {
  const value = new File([Uint8Array.from(bytes)], name, { type: "image/x-portable-graymap" });
  Object.defineProperty(value, "arrayBuffer", { value: vi.fn(async () => Uint8Array.from(bytes).buffer) }); return value;
}
function upload(value = file()) { fireEvent.change(screen.getByLabelText("导入离线原图"), { target: { files: [value] } }); }

describe("离线原图入口", () => {
  it("读取期间锁定，切换工位或配方后丢弃旧文件结果", async () => {
    const pending = deferred<ArrayBuffer>(); const value = file(); vi.mocked(value.arrayBuffer).mockReturnValueOnce(pending.promise);
    const onImport = vi.fn(async () => {}), onError = vi.fn(); const page = render(<ImageImportButton scope="old" onImport={onImport} onError={onError}/>);
    upload(value); expect(screen.getByRole("button", { name: "正在导入…" })).toBeDisabled();
    page.rerender(<ImageImportButton scope="new" onImport={onImport} onError={onError}/>);
    await act(async () => pending.resolve(Uint8Array.from([1]).buffer)); expect(onImport).not.toHaveBeenCalled(); expect(onError).not.toHaveBeenCalled();
  });
  it("导入命令失败报告原因，读取期间重复选择只执行一次", async () => {
    const request = deferred<void>(), onError = vi.fn(); const onImport = vi.fn(() => request.promise);
    render(<ImageImportButton scope="test" onImport={onImport} onError={onError}/>);
    upload(); upload(); await waitFor(() => expect(onImport).toHaveBeenCalledTimes(1));
    await act(async () => request.reject(new Error("原图损坏"))); expect(onError).toHaveBeenCalledWith("Error: 原图损坏");
  });

});
