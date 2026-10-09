import { describe, expect, it } from "vitest";
import { canPublish, canSaveFrame, coverage, historyRecords, initialState, reducer, restorePreview, sceneState, validationChecks, validationSamples, sampleVerdict, type WorkflowState } from "../src/features/workflow/model";

describe("操作流程预览的状态规则", () => {
  it.each(["image", "revision", "failed"])("过期或失败的 %s 不能保存", cause => {
    const s = sceneState("teach-pass"), frame = s.frames[2];
    if (cause === "image") frame.imageId = 999;
    if (cause === "revision") frame.revision++;
    if (cause === "failed") frame.trial!.pass = false;
    expect(canSaveFrame(frame)).toBe(false);
    expect(reducer(s, { type: "save-frame" })).toBe(s);
  });

  it("取样、试测、保存完成后可验证发布，配置修改使验证失效", () => {
    let s = initialState();
    s = reducer(s, { type: "capture" }); s = reducer(s, { type: "trial" }); s = reducer(s, { type: "save-frame" });
    expect(validationChecks(s).every(c => c.pass)).toBe(true);
    s = reducer(s, { type: "validate" }); expect(canPublish(s)).toBe(true);
    const edited = reducer(s, { type: "frame-param", key: "contrast", value: 40 });
    expect(edited.validation.status).toBe("idle"); expect(canPublish(edited)).toBe(false);
    expect(edited.frames[2].trial).toBeNull(); expect(edited.frames[2].saved).toBe(false);
    expect(s.frames[2].saved).toBe(true);
  });

  it("历史取样保留原始备份，多次历史取样后仍能恢复原图与参数", () => {
    const original = initialState();
    let s = reducer(original, { type: "capture", history: true });
    s = reducer(s, { type: "frame-param", key: "contrast", value: 50 });
    s = reducer(s, { type: "capture", history: true }); s = reducer(s, { type: "restore-frame" });
    expect(s.frames[2].imageId).toBe(original.frames[2].imageId);
    expect(s.frames[2].params).toEqual(original.frames[2].params);
    expect(s.frames[2].source).toBe("camera"); expect(s.frames[2].backup).toBeNull();
    expect(s.frames[2].trial).toBeNull(); expect(s.frames[2].saved).toBe(false);
  });

  it("采集参数修改清空示教，设备连接状态改变不清空", () => {
    const s = sceneState("validation-pass");
    expect(reducer(s, { type: "device", patch: { connected: false } }).frames).toBe(s.frames);
    const next = reducer(s, { type: "device", patch: { exposure: 80 } });
    expect(next.device.applied).toBe(false); expect(next.frames.every(f => f.imageId === null && !f.saved && !f.trial)).toBe(true);
    expect(next.validation.status).toBe("idle");
  });

  it("工件运行期间锁定采集参数与当前生产配置", () => {
    const s = sceneState("live-running"); s.recipe.candidate = s.recipe.production;
    expect(reducer(s, { type: "device", patch: { gain: 10 } })).toBe(s);
    expect(reducer(s, { type: "recipe", patch: { width: 600 } })).toBe(s);
  });

  it("物理覆盖不完整不能验证或发布", () => {
    const s = sceneState("validation-pass"); s.recipe.fovWidth = 170;
    expect(coverage(s)).toBeLessThan(100); expect(canPublish(s)).toBe(false);
    expect(reducer(s, { type: "validate" })).toBe(s);
  });

  it.each([NaN, Infinity, 1e12, 99])("非法工件尺寸 %s 快速拒绝覆盖规划", width => {
    const s = initialState(); s.recipe.width = width; expect(coverage(s)).toBe(0);
  });

  it.each(validationSamples.map(sample => [sample.name, sample] as const))("验证代表性样本：%s", (_name, sample) => {
    expect(sampleVerdict(initialState(), sample)).toBe(sample.expected);
  });

  it("放宽规则导致缺陷误判时验证失败", () => {
    const s = sceneState("validation-pass"); s.recipe.maxGap = 10;
    const next = reducer(s, { type: "validate" });
    expect(next.validation.status).toBe("failed"); expect(canPublish(next)).toBe(false);
  });

  it("发布在工件边界切换，当前工件与排队版本都保留独立快照", () => {
    let s = sceneState("validation-pass"); s = reducer(s, { type: "live-start" });
    const inFlight = s.live.inFlightConfig;
    s = reducer(s, { type: "publish" }); const queued = s.live.queuedConfig;
    expect(s.recipe.production).toBe(13); expect(s.live.queued).toBe(14); expect(canPublish(s)).toBe(false);
    s = reducer(s, { type: "frame-param", key: "contrast", value: 70 });
    expect(s.recipe.candidate).toBe(15); expect(queued?.frameParams[2].contrast).toBe(32);
    s = reducer(s, { type: "live-step" }); s = reducer(s, { type: "live-step" }); s = reducer(s, { type: "live-step" });
    expect(s.recipe.production).toBe(14); expect(s.productionConfig).toBe(queued); expect(s.live.queued).toBeNull();
    expect(s.live.inFlightConfig).toBe(inFlight); expect(s.live.inFlightVersion).toBe(13);
    s = reducer(s, { type: "live-start" }); expect(s.live.inFlightVersion).toBe(14);
  });

  it("停止接收新件仍完成当前工件", () => {
    let s = reducer(initialState(), { type: "live-start" }); s = reducer(s, { type: "live-stop" });
    expect(s.live.accepting).toBe(false); expect(s.live.phase).toBe(1);
    for (let i = 0; i < 3; i++) s = reducer(s, { type: "live-step" });
    expect(s.live.phase).toBe(4); expect(s.live.result).toBe("NG"); expect(s.live.accepting).toBe(false);
  });

  it.each(["device", "plc", "applied", "published"])("%s 未就绪不能启动", blocker => {
    const s = initialState();
    if (blocker === "device") s.device.connected = false;
    if (blocker === "plc") s.plc.ready = false;
    if (blocker === "applied") s.device.applied = false;
    if (blocker === "published") s.recipe.production = 0;
    expect(reducer(s, { type: "live-start" })).toBe(s);
  });

  it("无原图禁止重新测量，仍能规则重判，原记录不变", () => {
    const s = sceneState("history-no-raw"), original = structuredClone(historyRecords);
    expect(reducer(s, { type: "compare", kind: "remeasure" })).toBe(s);
    const next = reducer(s, { type: "compare", kind: "rejudge" });
    expect(next.comparisons[s.record].verdict).toBe("OK"); expect(historyRecords).toEqual(original);
  });

  it("随动模式需要全部相机标定和有效测量窗口", () => {
    const s = initialState(); s.recipe.mode = "follow";
    expect(validationChecks(s).every(c => c.pass)).toBe(false);
    s.follow.saved = [true, true, true]; expect(validationChecks(s).every(c => c.pass)).toBe(true);
    s.follow.params[1].near = s.follow.params[1].far;
    expect(validationChecks(s).every(c => c.pass)).toBe(false);
  });

  it("工作台独立保存几何、图像与验证，复制参数不复制保存证明，新配方从 v1 开始", () => {
    let s = sceneState("validation-pass");
    const original = structuredClone(s.frames);
    s = reducer(s, { type: "recipe", patch: { target: 3.1 } });
    s = reducer(s, { type: "validate" });
    s = reducer(s, { type: "recipe-create", name: " A 副本 ", mode: "fly", copy: true });
    expect(s.recipe).toMatchObject({ name: "A 副本", candidate: 1, production: 0, target: 3.1 });
    expect(s.frames.every(f => f.imageId === null && !f.saved && !f.trial && !f.backup)).toBe(true);
    s = reducer(s, { type: "frame-param", key: "contrast", value: 70 });
    s = reducer(s, { type: "recipe-open", name: "工件 A · 壳体" });
    expect(s.recipe.target).toBe(3.1); expect(s.frames).toEqual(original); expect(canPublish(s)).toBe(true);
    s = reducer(s, { type: "recipe-open", name: "A 副本" }); expect(s.frames[0].params.contrast).toBe(70);
    s = reducer(s, { type: "recipe-create", name: "新配方", mode: "follow", copy: false });
    expect(s.recipe).toMatchObject({ candidate: 1, production: 0, target: 3 });
    expect(s.frames[0].params.contrast).toBe(32); expect(s.recipeLibrary).toHaveLength(5);
  });

  it("无效或重复名字不能创建，未知配方不能打开，最后一项不能删除", () => {
    const s = initialState();
    for (const name of [" ", "工件 B · 底板", "工件 A · 壳体"]) expect(reducer(s, { type: "recipe-create", name, mode: "fly", copy: false })).toBe(s);
    expect(reducer(s, { type: "recipe-open", name: "missing" })).toBe(s);
    expect(reducer(s, { type: "recipe-delete", name: "missing" })).toBe(s);
    let next = reducer(s, { type: "recipe-delete", name: "工件 A · 壳体" });
    expect(next.recipe.name).toBe("工件 B · 底板"); expect(next.recipe.production).toBe(8);
    next = reducer(next, { type: "recipe-delete", name: "工件 C · 随动" });
    expect(reducer(next, { type: "recipe-delete", name: "工件 B · 底板" })).toBe(next);
  });

  it("运行中的配方和工位操作不会切换、删除或重标定本件", () => {
    const s = sceneState("live-running");
    expect(reducer(s, { type: "recipe-create", name: "new", mode: "fly", copy: false })).toBe(s);
    expect(reducer(s, { type: "recipe-open", name: "工件 B · 底板" })).toBe(s);
    expect(reducer(s, { type: "recipe-delete", name: s.recipe.name })).toBe(s);
    expect(reducer(s, { type: "plc", patch: { ready: false } })).toBe(s);
    expect(reducer(s, { type: "calibration", patch: { saved: true } })).toBe(s);
    expect(reducer(s, { type: "follow", patch: { camera: 2 } })).toBe(s);
    expect(reducer(s, { type: "live-option", patch: { scenario: "OK" } }).live.scenario).toBe("NG");
  });

  it("工位采集条件变化使所有工作台旧样本失效，生产快照保留", () => {
    const s = sceneState("validation-pass");
    const next = reducer(s, { type: "device", patch: { gain: 8 } });
    expect(next.recipeLibrary.every(entry => entry.frames.every(f => f.imageId === null && !f.saved && !f.trial) && entry.validation.status === "idle")).toBe(true);
    expect(next.recipeLibrary.map(entry => entry.productionConfig)).toEqual(s.recipeLibrary.map(entry => entry.productionConfig));
    expect(s.frames[0].imageId).toBe(41);
  });

  it("重复保存本帧或已保存的标定不反复增加候选修订", () => {
    const s = sceneState("teach-saved");
    expect(reducer(s, { type: "save-frame" })).toBe(s);
    expect(reducer(s, { type: "calibration", patch: { saved: true } })).toBe(s);
    const bad = sceneState("calibration-fail"); expect(reducer(bad, { type: "calibration", patch: { saved: true } })).toBe(bad);
  });

  it.each([0, 7, 2.5, NaN])("非法帧 %s 不污染当前选择", id => {
    const s = initialState(); expect(reducer(s, { type: "select-frame", id })).toBe(s);
  });

  it("无原图与缺失帧不能载入示教，有效帧仍保留原示教备份", () => {
    const s = sceneState("history-no-raw"); expect(reducer(s, { type: "capture", history: true })).toBe(s);
    const err = { ...initialState(), record: "TJ-000181" };
    expect(reducer(err, { type: "capture", history: true })).toBe(err);
    const other = reducer(err, { type: "select-frame", id: 4 });
    expect(reducer(other, { type: "capture", history: true }).frames[3].backup?.imageId).toBe(44);
  });

  it("连续新件使用已生效配置并清空旧结果，停止后不再自动开始", () => {
    let s = reducer(initialState(), { type: "live-option", patch: { continuous: true } });
    s = reducer(s, { type: "live-start" }); const first = s.live.part;
    for (let i = 0; i < 3; i++) s = reducer(s, { type: "live-step" });
    expect(s.live.result).toBe("NG"); const next = reducer(s, { type: "live-next" });
    expect(next.live).toMatchObject({ phase: 1, result: null, part: first + 1 }); expect(next.selectedFrame).toBe(1);
    const stopped = reducer(s, { type: "live-stop" }); expect(reducer(stopped, { type: "live-next" })).toBe(stopped);
    const paused = reducer(s, { type: "live-option", patch: { auto: false } }); expect(reducer(paused, { type: "live-next" })).toBe(paused);
  });

  it("旧预览存储迁移增加点位、配方库与连续选项，保留用户工作", () => {
    const s = structuredClone(sceneState("validation-pass")) as Partial<WorkflowState>;
    delete s.recipeLibrary; delete (s.plc as Partial<WorkflowState["plc"]>).points; delete (s.plc as Partial<WorkflowState["plc"]>).pointsApplied; delete (s.live as Partial<WorkflowState["live"]>).continuous;
    const restored = restorePreview(s);
    expect(restored.validation.status).toBe("passed"); expect(canPublish(restored)).toBe(true);
    expect(restored.plc.points).toHaveLength(6); expect(restored.recipeLibrary).toHaveLength(3); expect(restored.live.continuous).toBe(false);
  });

  it.each(["device", "frame", "trial", "follow", "runtime", "phase", "comparison", "library"])("嵌套存储损坏 %s 恢复初始状态", kind => {
    const s = structuredClone(sceneState("validation-pass"));
    if (kind === "device") s.device = null as unknown as WorkflowState["device"];
    if (kind === "frame") s.frames[2].params = null as unknown as WorkflowState["frames"][number]["params"];
    if (kind === "trial") s.frames[2].trial!.score = NaN;
    if (kind === "follow") s.follow.camera = 4;
    if (kind === "runtime") s.productionConfig.frameParams = [];
    if (kind === "phase") s.live.phase = 6;
    if (kind === "comparison") s.comparisons["TJ-000184"] = null as unknown as WorkflowState["comparisons"][string];
    if (kind === "library") s.recipeLibrary.push(s.recipeLibrary[0]);
    expect(restorePreview(s)).toEqual(initialState());
  });
});
