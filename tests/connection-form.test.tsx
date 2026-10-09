import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ConnectionForm from "../src/features/plc/components/ConnectionForm";
import type { ConnectionConfig, ProtocolKind } from "../src/features/plc/types";
import { plcConfig } from "./plc-fixtures";

const changed = vi.fn();
function Harness({ protocol }: { protocol: ProtocolKind }) {
  const initial = { ...plcConfig().connection, protocol, port: protocol === "s7" ? 102 : protocol === "mc" ? 5000 : 502 };
  const [value, setValue] = useState(initial);
  return <><ConnectionForm value={value} onChange={next => { changed(next); setValue(next); }} />
    <button onClick={() => setValue(initial)}>还原连接</button></>;
}
const latest = () => changed.mock.calls.at(-1)![0] as ConnectionConfig;
const change = (name: string | RegExp, value: string, role = "textbox") => fireEvent.change(screen.getByRole(role, { name }), { target: { value } });
beforeEach(() => changed.mockReset());

describe("PLC 连接输入的完整数值与自动值", () => {
  it.each(["0x0100junk", "0x10000", "-1", "0x", "1.5", "FFFFFFFFFFFFFFFF"])("TSAP 拒绝完整无效输入 %s，保留原文用于修正", value => {
    render(<Harness protocol="s7" />);
    for (const [label, property] of [[/本地 TSAP/, "localTsap"], [/远端 TSAP/, "remoteTsap"]] as const) {
      const input = screen.getByRole("textbox", { name: label });
      fireEvent.change(input, { target: { value } }); fireEvent.blur(input);
      expect(input).toHaveValue(value); expect(input).toHaveAttribute("aria-invalid", "true");
      expect(Number.isNaN(latest().s7[property])).toBe(true);
    }
    expect(screen.getByRole("alert")).toHaveTextContent("S7 TSAP");
  });

  it("TSAP 接受两端边界和大小写，留空恢复自动并即时清除错误", () => {
    render(<Harness protocol="s7" />);
    change(/本地 TSAP/, "0x0000"); change(/远端 TSAP/, "fFfF");
    expect(latest().s7).toMatchObject({ localTsap: 0, remoteTsap: 65535 });
    fireEvent.blur(screen.getByRole("textbox", { name: /远端 TSAP/ }));
    expect(screen.getByRole("textbox", { name: /远端 TSAP/ })).toHaveValue("0xFFFF");
    change(/本地 TSAP/, "garbage"); expect(screen.getByRole("alert")).toHaveTextContent("S7 TSAP");
    change(/本地 TSAP/, ""); change(/远端 TSAP/, "");
    expect(latest().s7).toMatchObject({ localTsap: null, remoteTsap: null }); expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each(["0x03FFbad-tail", "0x10000", "-1", "", "0x", "Infinity"])("MC IO 拒绝 %s，不静默还原默认模块", value => {
    render(<Harness protocol="mc" />); change("目标模块 IO 号", value);
    const input = screen.getByRole("textbox", { name: /目标模块 IO 号/ }); fireEvent.blur(input);
    expect(input).toHaveValue(value); expect(input).toHaveAttribute("aria-invalid", "true");
    expect(Number.isNaN(latest().mc.moduleIo)).toBe(true); expect(screen.getByRole("alert")).toHaveTextContent("MC 模块 IO");
  });

  it("MC IO 接受 0 与最大值，修正输入和还原同步显示", async () => {
    render(<Harness protocol="mc" />); change("目标模块 IO 号", "0"); expect(latest().mc.moduleIo).toBe(0);
    change("目标模块 IO 号", "ffff"); expect(latest().mc.moduleIo).toBe(65535);
    change("目标模块 IO 号", "bad-tail"); await userEvent.click(screen.getByRole("button", { name: "还原连接" }));
    expect(screen.getByRole("textbox", { name: "目标模块 IO 号" })).toHaveValue("0x03FF"); expect(screen.queryByRole("alert")).toBeNull();
  });

  it.each(["", "-1", "0", "65536", "502.5", "1e30"])("端口 %s 不成为可用端口", value => {
    render(<Harness protocol="modbusTcp" />); change("端口", value, "spinbutton");
    expect(screen.getByRole("spinbutton", { name: "端口" })).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("端口需为 1–65535 的整数");
    if (!value) { expect(screen.getByRole("spinbutton", { name: "端口" })).toHaveValue(null); expect(Number.isNaN(latest().port)).toBe(true); }
  });

  it("端口接受 1 与 65535，切回模拟器恢复无需端口的默认值", async () => {
    render(<Harness protocol="modbusTcp" />); change("端口", "1", "spinbutton"); expect(latest().port).toBe(1);
    change("端口", "65535", "spinbutton"); expect(latest().port).toBe(65535); expect(screen.queryByRole("alert")).toBeNull();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "协议" }), "simulator");
    expect(latest().port).toBe(0); expect(screen.getByRole("spinbutton", { name: "端口" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "IP / 主机名" })).toBeDisabled();
  });

  it("数值为空时不转换成零，低于后端最小轮询和超时的输入提示原因", () => {
    render(<Harness protocol="simulator" />);
    change("通讯超时 (ms)", "", "spinbutton"); expect(Number.isNaN(latest().timeoutMs)).toBe(true);
    expect(screen.getByRole("spinbutton", { name: "通讯超时 (ms)" })).toHaveValue(null);
    change("通讯超时 (ms)", "99", "spinbutton"); expect(screen.getByRole("alert")).toHaveTextContent("100 ms");
    change("通讯超时 (ms)", "100", "spinbutton"); change("轮询周期 (ms)", "19", "spinbutton");
    expect(screen.getByRole("alert")).toHaveTextContent("20 ms");
    change("轮询周期 (ms)", "20", "spinbutton"); change("重连间隔 (ms)", "0", "spinbutton");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("选择相同连接参数的 1500 型号时仍保留用户所选型号", async () => {
    render(<Harness protocol="s7" />); const cpu = screen.getByRole("combobox", { name: "CPU 型号" });
    await userEvent.selectOptions(cpu, "s71500"); expect(cpu).toHaveValue("s71500");
    expect(latest().s7).toMatchObject({ rack: 0, slot: 1, localTsap: null, remoteTsap: null });
    await userEvent.selectOptions(cpu, "s71200"); expect(cpu).toHaveValue("s71200");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
