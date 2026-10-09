import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { historyRecords, initialState, sceneState, type Verdict } from "../src/features/workflow/model";
import { click, finishTask, navigate, number, panel, select, showWorkflow, stored } from "./workflow-preview-fixtures";

beforeEach(() => { sessionStorage.clear(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("预览样本验证与发布", () => {
  it("缺项禁止验证发布，点击缺项定位未保存帧，完成后逐个验证样本并可取消或确认发布", async () => {
    showWorkflow("validation");
    expect(screen.getByRole("button", { name: "运行批量验证" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "发布候选配方" })).toBeDisabled();
    click(/所有帧示教已保存/); expect(screen.getByRole("button", { name: "选择帧 k3" })).toHaveAttribute("aria-pressed", "true");
    click("试匹配当前帧"); await finishTask(); click("保存本帧示教"); navigate("验证与发布");
    click("运行批量验证"); expect(screen.getByRole("button", { name: "运行批量验证" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "发布候选配方" })).toBeDisabled(); await finishTask();
    const table = screen.getByRole("table");
    for (const sample of ["正常胶路", "断胶样本", "位置偏离", "胶宽超限", "定位失败"]) {
      expect(within(screen.getByText(sample, { selector: "td strong" }).closest("tr")!).getByText("一致")).toBeVisible();
    }
    expect(within(table).getAllByRole("row")).toHaveLength(6);
    click("发布候选配方"); click("取消"); expect(stored().recipe.production).toBe(13);
    click("发布候选配方"); click("确认发布"); expect(stored().recipe.production).toBe(14);
    expect(screen.getByText("生产配方 v14 已生效")).toBeVisible();
    expect(screen.getByRole("button", { name: "发布候选配方" })).toBeDisabled();
    click("进入在线检测"); expect(screen.getByRole("heading", { level: 1, name: "在线检测" })).toBeVisible();
  });

  it("宽松判定造成样本不一致，修正规则后重新验证", async () => {
    showWorkflow("geometry", sceneState("validation-pass")); number("允许断胶长度", 10); navigate("验证与发布");
    click("运行批量验证"); await finishTask();
    expect(screen.getByText("验证结果不符合样本预期")).toBeVisible();
    const defect = screen.getByText("断胶样本", { selector: "td strong" }).closest("tr")!;
    expect(within(defect).getByText("不一致")).toBeVisible(); expect(screen.getByRole("button", { name: "发布候选配方" })).toBeDisabled();
    navigate("胶路与拍照规划"); number("允许断胶长度", .5); navigate("验证与发布");
    expect(within(panel("代表性样本验证")).getAllByText("待验证")).toHaveLength(5);
    click("运行批量验证"); await finishTask(); expect(screen.getByRole("button", { name: "发布候选配方" })).toBeEnabled();
  });

  it("本件运行中确认发布只排队，继续编辑候选不改变本件和已排队版本", () => {
    const state = sceneState("validation-pass"); state.live.phase = 2; state.live.accepting = true; state.live.inFlightConfig = state.productionConfig; state.live.inFlightVersion = 13;
    showWorkflow("validation", state); click("发布候选配方");
    expect(screen.getByText("当前工件完成后生效")).toBeVisible(); click("确认发布");
    expect(screen.getByRole("heading", { level: 1, name: "在线检测" })).toBeVisible();
    expect(stored().recipe.production).toBe(13); expect(stored().live.queued).toBe(14);
    navigate("胶路与拍照规划"); number("距内边基准 d", 3.5);
    expect(stored().recipe.candidate).toBe(15); expect(stored().live.queuedConfig?.recipe.target).toBe(3);
    navigate("在线检测"); click("推进一次预览事件"); click("推进一次预览事件");
    expect(stored().recipe.production).toBe(14); expect(stored().productionConfig.recipe.target).toBe(3);
    expect(stored().live.inFlightVersion).toBe(13); expect(stored().live.queued).toBeNull();
    click("开始下一件"); expect(stored().live.inFlightVersion).toBe(14); expect(stored().live.result).toBeNull();
  });

  it("离开待批量验证页面取消旧验证，发布弹窗可从关闭键、背景和取消事件退出", async () => {
    showWorkflow("validation", sceneState("validation-pass")); click("运行批量验证"); navigate("单帧示教"); await finishTask();
    navigate("验证与发布");
    click("发布候选配方"); click("关闭弹窗"); expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    click("发布候选配方"); fireEvent.click(screen.getByRole("dialog")); expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    click("发布候选配方"); fireEvent(screen.getByRole("dialog"), new Event("cancel", { bubbles: true, cancelable: true }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(); expect(stored().recipe.production).toBe(13);
  });
});

describe("预览在线操作", () => {
  it.each(["正常胶路 · OK", "断胶超限 · NG", "原图缺失 · ERR"])("启动样本 %s、暂停自动、逐事件完成，并清空下一件的旧结果", async sample => {
    showWorkflow("live"); select("下一件样本", sample); click("启动检测");
    const part = stored().live.part;
    expect(screen.getByRole("combobox", { name: "下一件样本" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "启动检测" })).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: "自动演示事件" })); await finishTask(5000);
    expect(stored().live.phase).toBe(1);
    for (let phase = 2; phase <= 4; phase++) { click("推进一次预览事件"); expect(stored().live.phase).toBe(phase); }
    const result = sample.split(" · ")[1] as Verdict;
    expect(stored().live.result).toBe(result); expect(within(panel("整件判定")).getByText(result, { selector: ".wf-result-card strong" })).toBeVisible();
    if (result === "NG") {
      click("选择帧 k4"); expect(screen.getByRole("img", { name: "帧 k4 的采集原图" })).toBeVisible();
      expect(within(panel("整件判定")).getByText("NG", { selector: ".wf-result-card strong" })).toBeVisible();
      click("定位缺陷帧"); expect(screen.getByRole("button", { name: "选择帧 k3" })).toHaveAttribute("aria-pressed", "true");
      expect(screen.getByRole("img", { name: "帧 k3 的采集原图，含断胶标记" })).toBeVisible();
    }
    if (result === "ERR") {
      click("选择帧 k3"); expect(screen.getByText("该帧原图不可用")).toBeVisible();
      expect(screen.getByText("测量数据无效")).toBeVisible(); click("总览帧 k4"); expect(screen.getByRole("img", { name: "帧 k4 的采集原图" })).toBeVisible();
    }
    click("开始下一件"); expect(stored().live.part).toBe(part + 1); expect(stored().selectedFrame).toBe(1); expect(stored().live.result).toBeNull();
    expect(screen.queryByText("测量数据无效")).not.toBeInTheDocument(); expect(screen.queryByRole("button", { name: "定位缺陷帧" })).not.toBeInTheDocument();
    expect(within(panel("整件判定")).getByText("检测中", { selector: ".wf-result-card strong" })).toBeVisible();
  });

  it("自动连续预览依次完成和开始下一件，本件后停止保留结论而不再启动", async () => {
    showWorkflow("live"); fireEvent.click(screen.getByRole("checkbox", { name: "连续预览" })); click("启动检测");
    const first = stored().live.part;
    for (let i = 0; i < 3; i++) await finishTask(1600);
    expect(stored().live).toMatchObject({ phase: 4, result: "NG", accepting: true, part: first });
    await finishTask(1600); expect(stored().live).toMatchObject({ phase: 1, result: null, part: first + 1 });
    click("本件后停止"); expect(screen.getByText("已停止接收新工件，当前工件继续完成")).toBeVisible();
    for (let i = 0; i < 3; i++) await finishTask(1600);
    expect(stored().live).toMatchObject({ phase: 4, result: "NG", accepting: false });
    await finishTask(10000); expect(stored().live.part).toBe(first + 1);
  });

  it("连续运行完成后可取消下一件，自动事件可恢复，缺陷入口回放原始示例记录", async () => {
    showWorkflow("live"); fireEvent.click(screen.getByRole("checkbox", { name: "连续预览" })); click("启动检测");
    fireEvent.click(screen.getByRole("checkbox", { name: "自动演示事件" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "自动演示事件" }));
    for (let i = 0; i < 3; i++) await finishTask(1600);
    click("停止接件"); await finishTask(5000); expect(stored().live.phase).toBe(4);
    click("历史复盘"); expect(stored().record).toBe("TJ-000184"); expect(screen.getByRole("heading", { level: 1, name: "历史复测" })).toBeVisible();
  });

  it.each(["device", "applied", "plc", "published"])("%s 未就绪不能启动", blocker => {
    const s = initialState();
    if (blocker === "device") s.device.connected = false;
    if (blocker === "applied") s.device.applied = false;
    if (blocker === "plc") s.plc.ready = false;
    if (blocker === "published") s.recipe.production = 0;
    showWorkflow("live", s); expect(screen.getByRole("button", { name: "启动检测" })).toBeDisabled(); expect(screen.getByText("启动条件未满足")).toBeVisible();
  });

  it("随动工况使用本件生产快照与偏移曲线，选帧不改变整件工况", () => {
    const s = sceneState("live-ng"); s.productionConfig.recipe.mode = "follow"; s.live.inFlightConfig = s.productionConfig;
    showWorkflow("live", s); expect(screen.getByText("横向偏移")).toBeVisible(); click("选择帧 k5");
    expect(screen.getByText("横向偏移")).toBeVisible(); expect(stored().live.result).toBe("NG");
  });
});

describe("预览历史筛选与对照", () => {
  it("SN、日期、结果、原图筛选与清空，空结果不可导出，记录入口定位缺陷帧", () => {
    showWorkflow("history");
    fireEvent.change(screen.getByRole("textbox", { name: "工件 SN" }), { target: { value: "000184" } });
    expect(screen.getByText("1 件记录 · 原始检测结果")).toBeVisible();
    select("检测结果", "OK"); expect(screen.getByText("没有匹配的历史记录")).toBeVisible(); expect(screen.getByRole("button", { name: "导出当前筛选" })).toBeDisabled();
    click("清空"); select("原图状态", "原图已清理"); expect(screen.getByText("TJ-000182", { selector: "td strong" })).toBeVisible();
    click("清空"); fireEvent.change(screen.getByLabelText("开始日期"), { target: { value: "2026-10-08" } }); fireEvent.change(screen.getByLabelText("结束日期"), { target: { value: "2026-10-07" } });
    expect(screen.getByText("开始日期不能晚于结束日期")).toBeVisible(); click("清空");
    select("检测结果", "ERR"); select("原图状态", "原图可用"); click("查看记录");
    expect(stored().record).toBe("TJ-000181"); expect(stored().selectedFrame).toBe(3);
    expect(screen.getByRole("button", { name: "用作本帧示教样本" })).toBeDisabled();
    click("选择帧 k4"); expect(screen.getByRole("button", { name: "用作本帧示教样本" })).toBeEnabled();
    click("返回记录"); expect(screen.getByRole("heading", { level: 1, name: "历史记录" })).toBeVisible();
  });

  it("导出仅包含筛选记录，失败可重试，撤销临时下载地址", async () => {
    let blob: Blob | undefined;
    const create = vi.fn((value: Blob) => { blob = value; return "blob:workflow"; }); const revoke = vi.fn();
    vi.stubGlobal("URL", class extends URL { static createObjectURL = create; static revokeObjectURL = revoke; });
    const download = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    showWorkflow("history"); select("检测结果", "NG");
    create.mockImplementationOnce(() => { throw new Error("storage unavailable"); });
    click("导出当前筛选"); expect(screen.getByText("导出失败，请重试。")).toBeVisible(); click("导出当前筛选");
    expect(download).toHaveBeenCalledOnce(); expect(screen.queryByText("导出失败，请重试。")).not.toBeInTheDocument();
    expect(blob?.type).toBe("text/csv;charset=utf-8");
    const reader = new FileReader(); const result = new Promise<string>(resolve => { reader.onload = () => resolve(String(reader.result)); }); reader.readAsText(blob!);
    await finishTask(1000); const exported = await result;
    expect(exported).toContain("TJ-000184"); expect(exported).not.toContain("TJ-000183"); await finishTask(1000); expect(revoke).toHaveBeenCalledWith("blob:workflow");
  });

  it("规则重判与原图复测生成独立对照，原始结果和原图保留", async () => {
    const original = structuredClone(historyRecords); showWorkflow("record");
    click("按候选规则重判"); expect(screen.getByRole("button", { name: "用原图重新测量" })).toBeDisabled(); await finishTask();
    expect(stored().comparisons["TJ-000184"]).toMatchObject({ kind: "rejudge", verdict: "NG", gap: 6.2, version: 14 });
    click("用原图重新测量"); await finishTask();
    expect(stored().comparisons["TJ-000184"]).toMatchObject({ kind: "remeasure", verdict: "OK", gap: 0, version: 14 });
    expect(within(panel("原始检测")).getByText("NG")).toBeVisible(); expect(within(panel("候选对照")).getByText("OK")).toBeVisible();
    click("总览帧 k4"); expect(screen.getByRole("img", { name: "帧 k4 的历史原图" })).toBeVisible();
    expect(historyRecords).toEqual(original);
  });

  it("无原图仍可规则重判、不能复测和取样；离开页面取消待对照", async () => {
    showWorkflow("record?scene=history-no-raw"); expect(screen.getByText("原图已按留存策略清理")).toBeVisible();
    expect(screen.getByRole("button", { name: "用原图重新测量" })).toBeDisabled(); expect(screen.getByRole("button", { name: "用作本帧示教样本" })).toBeDisabled();
    click("按候选规则重判"); await finishTask(); expect(stored().comparisons["TJ-000182"].verdict).toBe("OK");
    click("按候选规则重判"); click("返回记录"); await finishTask(); expect(stored().comparisons["TJ-000182"].kind).toBe("rejudge");
  });
});
