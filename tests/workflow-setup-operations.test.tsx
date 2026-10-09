import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialState, sceneState } from "../src/features/workflow/model";
import { click, finishTask, navigate, number, select, showWorkflow, stored } from "./workflow-preview-fixtures";

beforeEach(() => { sessionStorage.clear(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("预览建站操作", () => {
  it("连接、编辑全部采集条件、应用、取帧，并在条件改变后撤下旧预览", async () => {
    const state = initialState(); state.device.connected = false; state.device.applied = false;
    showWorkflow("device", state);
    expect(screen.getByRole("button", { name: "取一帧" })).toBeDisabled();
    click("连接相机");
    number("曝光时间", 9);
    expect(screen.getByRole("button", { name: "保存并应用参数" })).toBeDisabled();
    fireEvent.blur(screen.getByRole("spinbutton", { name: "曝光时间" }));
    expect(screen.getByRole("spinbutton", { name: "曝光时间" })).toHaveValue(10);
    number("增益", 25);
    expect(screen.getByRole("button", { name: "保存并应用参数" })).toBeDisabled();
    fireEvent.blur(screen.getByRole("spinbutton", { name: "增益" }));
    number("曝光时间", 80); number("增益", 5.5); select("触发源", "软件触发");
    click("保存并应用参数"); click("保存并应用参数");
    expect(screen.getByRole("spinbutton", { name: "曝光时间" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "断开" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取一帧" })).toBeDisabled();
    await finishTask();
    expect(stored().device).toMatchObject({ connected: true, applied: true, exposure: 80, gain: 5.5, trigger: "软件触发" });
    expect(stored().frames.every(f => f.imageId === null && !f.saved)).toBe(true);
    click("取一帧");
    expect(screen.getByRole("img", { name: "帧 k3 的采集预览" })).toBeVisible();
    number("曝光时间", 90);
    expect(screen.queryByRole("img", { name: "帧 k3 的采集预览" })).not.toBeInTheDocument();
    click("保存并应用参数"); await finishTask(); click("取一帧"); click("断开");
    expect(screen.queryByRole("img", { name: "帧 k3 的采集预览" })).not.toBeInTheDocument();
    click("下一步 · PLC 通讯"); expect(screen.getByRole("heading", { level: 1, name: "PLC 通讯" })).toBeVisible();
  });

  it("离开等待中的设备页面后不应用旧参数", async () => {
    showWorkflow("device"); number("曝光时间", 100); click("保存并应用参数");
    navigate("PLC 通讯"); await finishTask();
    expect(stored().device.applied).toBe(false);
    navigate("设备与采集"); expect(screen.getByRole("button", { name: "保存并应用参数" })).toBeEnabled();
  });

  it("PLC 校验地址、保存六个点位、检查握手，并在换协议后重新应用", () => {
    showWorkflow("plc");
    fireEvent.change(screen.getByRole("textbox", { name: "PLC 地址" }), { target: { value: "192.168.1.999" } });
    expect(screen.getByRole("textbox", { name: "PLC 地址" })).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "连接 PLC" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "PLC 地址" }), { target: { value: "192.168.2.10" } });
    click("连接 PLC");
    const signals = ["开始信号", "相机布防", "采集结束", "结果就绪", "结果码", "PLC 确认"];
    signals.forEach((signal, i) => fireEvent.change(screen.getByRole("textbox", { name: signal + "地址" }), { target: { value: "DB30." + i } }));
    expect(screen.getByRole("button", { name: "检查业务握手" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "开始信号地址" }), { target: { value: " " } });
    expect(screen.getByRole("button", { name: "应用点位" })).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "开始信号地址" }), { target: { value: " DB30.0 " } });
    click("应用点位"); click("检查业务握手");
    expect(stored().plc).toMatchObject({ ready: true, pointsApplied: true, points: signals.map((_, i) => "DB30." + i) });
    expect(screen.getByText("14:32:02 握手就绪")).toBeVisible();
    navigate("设备与采集"); navigate("PLC 通讯");
    expect(screen.getByRole("textbox", { name: "开始信号地址" })).toHaveValue("DB30.0");
    select("通讯协议", "Modbus TCP");
    expect(stored().plc).toMatchObject({ connected: false, ready: false, pointsApplied: false });
    expect(screen.getByRole("textbox", { name: "开始信号地址" })).toHaveValue("00001");
    expect(screen.getByText("502 / 1")).toBeVisible();
    click("应用点位"); click("连接 PLC"); click("检查业务握手"); click("断开连接");
    expect(stored().plc).toMatchObject({ connected: false, ready: false });
    click("下一步 · 工位标定"); expect(screen.getByRole("heading", { level: 1, name: "飞拍工位标定" })).toBeVisible();
  });

  it("遮挡棋盘失败不能保存，重新取完整棋盘成功后更新标定并失效示教", async () => {
    const state = initialState(); state.calibration.captured = false; state.calibration.saved = false; state.calibration.result = "idle";
    showWorkflow("calibration", state);
    expect(screen.getByRole("button", { name: "计算标定" })).toBeDisabled();
    select("预览样本", "遮挡棋盘格"); click("取标定帧"); click("计算标定");
    expect(screen.getByRole("combobox", { name: "预览样本" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取标定帧" })).toBeDisabled();
    await finishTask();
    expect(screen.getByText("未完整识别标定角点")).toBeVisible();
    expect(screen.getByRole("button", { name: "保存标定" })).toBeDisabled();
    select("预览样本", "完整棋盘格"); click("取标定帧"); click("计算标定"); await finishTask(); click("保存标定");
    expect(screen.getByText("有效标定 C08")).toBeVisible();
    expect(stored().frames.every(f => f.imageId === null && !f.saved && !f.trial)).toBe(true);
    expect(screen.getByRole("button", { name: "保存标定" })).toBeDisabled();
    click("进入配方库"); expect(screen.getByRole("heading", { level: 1, name: "配方库" })).toBeVisible();
  });

  it("计算标定时离开页面会取消旧结果", async () => {
    showWorkflow("calibration?scene=calibration-fail"); select("预览样本", "完整棋盘格"); click("计算标定");
    navigate("设备与采集"); await finishTask(); expect(stored().calibration.result).toBe("idle");
  });

  it("逐台修改随动参数、冻结试测和保存，切换相机不混用试测", async () => {
    showWorkflow("follow");
    expect(screen.getByRole("button", { name: "试测当前相机" })).toBeDisabled();
    for (let camera = 1; camera <= 3; camera++) {
      click("相机 " + camera);
      expect(screen.getByRole("button", { name: "保存本相机" })).toBeDisabled();
      number("胶嘴基准位置", 130 + camera); number("像素比例", .05); select("运动方向", ["向右", "向左", "向上"][camera - 1]);
      number("近端距离", 30); number("远端距离", 20); click("冻结当前帧");
      expect(screen.getByText("测量窗口无效")).toBeVisible();
      expect(screen.getByRole("button", { name: "试测当前相机" })).toBeDisabled();
      number("远端距离", 110); click("试测当前相机");
      expect(screen.getByRole("button", { name: "相机 2" })).toBeDisabled();
      expect(screen.getByRole("spinbutton", { name: "像素比例" })).toBeDisabled();
      await finishTask(); click("保存本相机");
      expect(screen.getByRole("button", { name: "保存本相机" })).toBeDisabled();
    }
    expect(screen.getByText("3 / 3 台已保存")).toBeVisible();
    click("相机 1"); number("胶嘴基准位置", 140);
    expect(stored().follow.saved).toEqual([false, true, true]);
    expect(stored().follow.params[1]).toMatchObject({ nozzle: 132, direction: "向左" });
    click("进入随动配方"); expect(stored().recipe.mode).toBe("follow");
  });

  it.each(["device", "plc", "calibration", "follow"])("本件运行中锁定 %s 建站操作", view => {
    showWorkflow(view, sceneState("live-running"));
    for (const input of screen.queryAllByRole("spinbutton")) expect(input).toBeDisabled();
    for (const input of screen.queryAllByRole("combobox")) expect(input).toBeDisabled();
    const actions: Record<string, string[]> = { device: ["断开", "取一帧", "保存并应用参数"], plc: ["断开连接", "应用点位", "检查业务握手"], calibration: ["取标定帧", "计算标定", "保存标定"], follow: ["冻结当前帧", "试测当前相机", "保存本相机"] };
    actions[view].forEach(name => expect(screen.getByRole("button", { name })).toBeDisabled());
  });
});

describe("预览设置与情景操作", () => {
  it.each([["原图保留天数", 0], ["原图保留天数", 366], ["原图保留天数", 1.5], ["等待工件完成超时", 99], ["等待工件完成超时", 60001], ["等待工件完成超时", 100.5]] as const)("%s 的非法值 %s 不可保存", (label, value) => {
    showWorkflow("settings"); number(label, value); expect(screen.getByRole("button", { name: "保存设置" })).toBeDisabled(); expect(stored().settings.saved).toBe(false);
  });

  it.each(["全部工件", "NG 与 ERR", "仅 ERR"])("保存全部设置与策略 %s，换页后保持，诊断可操作", strategy => {
    showWorkflow("settings"); number("原图保留天数", 90); number("等待工件完成超时", 8000); select("原图保存策略", strategy); click("保存设置");
    expect(stored().settings).toEqual({ retention: 90, timeout: 8000, raw: strategy, saved: true });
    expect(screen.getByRole("button", { name: "保存设置" })).toBeDisabled();
    navigate("设备与采集"); navigate("系统设置"); expect(screen.getByRole("spinbutton", { name: "原图保留天数" })).toHaveValue(90);
    click("诊断信息"); click("运行诊断检查"); expect(screen.getByRole("status")).toHaveTextContent("诊断检查完成");
    click("运行配置"); expect(screen.getByRole("spinbutton", { name: "等待工件完成超时" })).toHaveValue(8000);
  });

  it("情景分类、查询、空结果和清空筛选都可操作，关闭保持原状态", () => {
    showWorkflow("teach"); const before = stored(); click("全部情景");
    const dialog = screen.getByRole("dialog");
    select("情景分类", "异常");
    expect(within(dialog).getByRole("button", { name: /搜索窗口覆盖不足/ })).toBeVisible();
    expect(within(dialog).queryByRole("button", { name: /冻结图像/ })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "查找情景" }), { target: { value: "没有这条情景" } });
    expect(screen.getByText("没有匹配的情景")).toBeVisible(); click("清空筛选");
    expect(screen.getByRole("textbox", { name: "查找情景" })).toHaveValue("");
    expect(screen.getByRole("combobox", { name: "情景分类" })).toHaveValue("全部");
    expect(within(dialog).getAllByRole("button")).toHaveLength(34);
    click("关闭弹窗"); expect(stored()).toEqual(before);
  });
});
