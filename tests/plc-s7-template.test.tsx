import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import S7Phase1Dialog from "../src/features/plc/components/S7Phase1Dialog";
import { plcApi } from "../src/features/plc/api";
import { deferred } from "./fixtures";
import { plcConfig } from "./plc-fixtures";
import type { PlcConfig } from "../src/features/plc/types";

vi.mock("../src/features/plc/api", () => ({ plcApi: { s7Phase1Template: vi.fn() } }));
const template = (db: number) => {
  const config = plcConfig();
  config.connection.protocol = "s7"; config.connection.port = 102;
  config.points = [{ ...config.points[0], id: "preset", name: "后端预设点", address: `DB${db}.DBW12`, dataType: "u16", tags: ["protocolVersion"] }];
  config.heartbeat.pointId = "preset";
  return config;
};
beforeEach(() => { vi.mocked(plcApi.s7Phase1Template).mockImplementation(db => Promise.resolve(template(db))); });
const dbInput = () => screen.getByRole("spinbutton", { name: "DB 号" });
const applyButton = () => screen.getByRole("button", { name: "应用到草稿" });

describe("一期 S7 模板预览", () => {
  it.each(["s71200", "s71500"])("%s 采用后端地址并保留当前草稿的现场设置", async cpu => {
    const config = plcConfig(); config.connection.host = "10.1.2.3"; config.connection.pollIntervalMs = 35;
    config.logRetentionDays = 9; config.autoConnect = true;
    const before = structuredClone(config), onApply = vi.fn(), onClose = vi.fn();
    render(<S7Phase1Dialog config={config} dirty blockedReason="" onApply={onApply} onClose={onClose} />);
    expect(dbInput()).toHaveValue(100);
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "一期 CPU 预设" }), cpu);
    expect(await screen.findByText("DB100.DBW12")).toBeVisible();
    expect(applyButton()).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(applyButton());
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      connection: expect.objectContaining({ host: "10.1.2.3", pollIntervalMs: 35, protocol: "s7", port: 102,
        s7: expect.objectContaining({ rack: 0, slot: 1, localTsap: null, remoteTsap: null }) }),
      points: template(100).points, heartbeat: template(100).heartbeat, logRetentionDays: 9, autoConnect: true,
    }), cpu);
    expect(config).toEqual(before); expect(onClose).not.toHaveBeenCalled();
  });

  it("现有有效 DB 自动带入，取消保留当前配置", async () => {
    const config = plcConfig(); config.points[0].address = "DB65000.DBD20";
    const onApply = vi.fn(), onClose = vi.fn();
    render(<S7Phase1Dialog config={config} dirty blockedReason="" onApply={onApply} onClose={onClose} />);
    expect(dbInput()).toHaveValue(65000);
    await userEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onClose).toHaveBeenCalledTimes(1); expect(onApply).not.toHaveBeenCalled();
  });

  it.each(["", "0", "65536", "1.5", "-1"])("DB 号 %s 无效时不请求点表也不允许应用", async value => {
    const onApply = vi.fn();
    render(<S7Phase1Dialog config={plcConfig()} dirty={false} blockedReason="" onApply={onApply} onClose={vi.fn()} />);
    fireEvent.change(dbInput(), { target: { value } });
    expect(screen.getByRole("alert")).toHaveTextContent("DB 号需为 1–65535 的整数");
    await userEvent.click(screen.getByRole("checkbox"));
    expect(applyButton()).toBeDisabled(); expect(plcApi.s7Phase1Template).not.toHaveBeenCalled();
    expect(onApply).not.toHaveBeenCalled();
  });

  it("变更 DB 后忽略上一请求，并重新确认替换范围", async () => {
    const first = deferred<PlcConfig>();
    vi.mocked(plcApi.s7Phase1Template).mockReturnValueOnce(first.promise);
    const onApply = vi.fn();
    render(<S7Phase1Dialog config={plcConfig()} dirty={false} blockedReason="" onApply={onApply} onClose={vi.fn()} />);
    await waitFor(() => expect(plcApi.s7Phase1Template).toHaveBeenCalledWith(100));
    await userEvent.click(screen.getByRole("checkbox"));
    fireEvent.change(dbInput(), { target: { value: "200" } });
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    expect(await screen.findByText("DB200.DBW12")).toBeVisible();
    await act(async () => first.resolve(template(100)));
    expect(screen.queryByText("DB100.DBW12")).toBeNull();
    await userEvent.click(screen.getByRole("checkbox")); await userEvent.click(applyButton());
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ points: template(200).points }), "s71200");
  });

  it("生成失败可重试，未确认前不能覆盖现有表", async () => {
    vi.mocked(plcApi.s7Phase1Template).mockRejectedValueOnce(new Error("模板生成失败"));
    render(<S7Phase1Dialog config={plcConfig()} dirty={false} blockedReason="" onApply={vi.fn()} onClose={vi.fn()} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("模板生成失败"); expect(applyButton()).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findByText("DB100.DBW12")).toBeVisible(); expect(applyButton()).toBeDisabled();
  });

  it("预览完成后生产开始，保留预览但禁止应用，仍能取消", async () => {
    const onApply = vi.fn(), onClose = vi.fn(), config = plcConfig();
    const view = render(<S7Phase1Dialog config={config} dirty={false} blockedReason="" onApply={onApply} onClose={onClose} />);
    await screen.findByText("DB100.DBW12"); await userEvent.click(screen.getByRole("checkbox"));
    view.rerender(<S7Phase1Dialog config={config} dirty={false} blockedReason="生产事务未结束" onApply={onApply} onClose={onClose} />);
    expect(applyButton()).toBeDisabled(); expect(dbInput()).toBeDisabled();
    fireEvent.click(applyButton()); expect(onApply).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "取消" })); expect(onClose).toHaveBeenCalled();
  });
});
