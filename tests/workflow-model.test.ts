import { describe, expect, it } from "vitest";
import { canPublish, canSaveFrame, coverage, historyRecords, initialState, reducer, restorePreview, runtimeConfig, sceneState, validationChecks, validationSamples, sampleVerdict, type WorkflowState } from "../src/features/workflow/model";

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
    const edited = reducer(s, { type: "frame-param", key: "search", value: 5 });
    expect(edited.validation.status).toBe("idle"); expect(canPublish(edited)).toBe(false);
    expect(edited.frames[2].trial).toBeNull(); expect(edited.frames[2].saved).toBe(false);
    expect(s.frames[2].saved).toBe(true);
  });

  it("历史取样保留原始备份，多次历史取样后仍能恢复原图与参数", () => {
    const original = initialState();
    let s = reducer(original, { type: "capture", history: true });
    s = reducer(s, { type: "frame-param", key: "search", value: 6 });
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
    expect(next.device.applied).toBe(false); expect(next.frames.filter(f => f.camera === "cam1").every(f => f.imageId === null && !f.saved && !f.trial)).toBe(true);
    expect(next.frames.filter(f => f.camera !== "cam1")).toEqual(s.frames.filter(f => f.camera !== "cam1"));
    expect(next.validation.status).toBe("idle");
  });

  it("工件运行期间锁定采集参数与当前生产配置", () => {
    const s = sceneState("live-running"); s.recipe.candidate = s.recipe.production;
    expect(reducer(s, { type: "device", patch: { gain: 10 } })).toBe(s);
    expect(reducer(s, { type: "recipe", patch: { spacing: 2 } })).toBe(s);
  });

  it("原图外的中线不能验证或发布", () => {
    const s = sceneState("validation-pass"); s.frames[2].path[0] = [-1, 200];
    expect(coverage(s)).toBeLessThan(100); expect(canPublish(s)).toBe(false);
    expect(reducer(s, { type: "validate" })).toBe(s);
  });

  it.each([0, -1, NaN, Infinity, 11])("非法像素比例 %s 拒绝本点规划", mmPerPx => {
    const s = initialState(); s.frames[2].mmPerPx = mmPerPx; expect(coverage(s)).toBeLessThan(100);
  });

  it.each(["shotId", "poseId", "camera", "view", "path", "mmPerPx", "polarity"])("修改本点 %s 只撤销该点试测证明，冻结生产不变", field => {
    const s = sceneState("validation-pass"), others = s.frames.filter(f => f.id !== 3);
    const next = field === "path" ? reducer(s, { type: "frame-path", path: [[150, 200], [800, 600]] })
      : field === "mmPerPx" ? reducer(s, { type: "frame-scale", value: .1 })
      : field === "polarity" ? reducer(s, { type: "frame-polarity", value: "dark" })
      : reducer(s, { type: "frame-plan", patch: field === "shotId" ? { shotId: "P3-new" } : field === "poseId" ? { poseId: "Pose-new" } : field === "camera" ? { camera: "cam2" } : { view: 2 } });
    expect(next.frames[2]).toMatchObject({ trial: null, saved: false });
    expect(next.frames.filter(f => f.id !== 3)).toEqual(others); expect(next.productionConfig).toBe(s.productionConfig);
    expect(next.frames[2].imageId === null).toBe(["shotId", "poseId", "camera", "view"].includes(field));
  });

  it("三目视角属于单设备，重复 ID 和超范围视角不能发布", () => {
    const s = sceneState("validation-pass");
    expect(s.frames.slice(0, 3).map(f => [f.camera, f.view])).toEqual([["cam1", 1], ["cam1", 2], ["cam1", 3]]);
    expect(canPublish(s)).toBe(true);
    const duplicate = reducer(s, { type: "frame-plan", patch: { shotId: "P1" } });
    expect(coverage(duplicate)).toBeLessThan(100);
    expect(coverage(reducer(s, { type: "frame-plan", patch: { camera: "cam2", view: 3 } }))).toBe(100);
    const wrongView = reducer(reducer(s, { type: "frame-plan", patch: { camera: "cam2" } }), { type: "frame-plan", patch: { view: 3 } });
    expect(coverage(wrongView)).toBeLessThan(100);
  });

  it("明确不检逐点保存，不需要伪造试测通过", () => {
    let s = reducer(initialState(), { type: "frame-plan", patch: { skip: true } });
    s = reducer(s, { type: "save-frame" });
    expect(s.frames[2]).toMatchObject({ skip: true, imageId: null, trial: null, saved: true });
    expect(validationChecks(s).every(c => c.pass)).toBe(true);
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
    s = reducer(s, { type: "frame-param", key: "search", value: 8 });
    expect(s.recipe.candidate).toBe(15); expect(queued?.shots[2].params.search).toBe(4);
    s = reducer(s, { type: "live-step" }); s = reducer(s, { type: "live-step" }); s = reducer(s, { type: "live-step" });
    expect(s.recipe.production).toBe(14); expect(s.productionConfig).toBe(queued); expect(s.live.queued).toBeNull();
    expect(s.live.inFlightConfig).toBe(inFlight); expect(s.live.inFlightVersion).toBe(13);
    s = reducer(s, { type: "live-start" }); expect(s.live.inFlightVersion).toBe(14);
  });

  it("发布身份使用明确版本，资源内容变化不会生成指纹", () => {
    const published = reducer(sceneState("validation-pass"), { type: "publish" });
    expect(runtimeConfig(published.recipe, published.frames, published.overview, published.recipe.production).bundleId).toBe(published.productionConfig.bundleId);
    const recaptured = reducer(published, { type: "capture" });
    const next = runtimeConfig(recaptured.recipe, recaptured.frames, recaptured.overview, published.recipe.production);
    expect(next.recipeRevision).toBe(published.productionConfig.recipeRevision); expect(next.bundleId).toBe(published.productionConfig.bundleId);
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

  it("工作台独立保存逐点中线、图像与验证，复制参数不复制保存证明，新配方从 v1 开始", () => {
    let s = sceneState("validation-pass");
    const original = structuredClone(s.frames);
    s = reducer(s, { type: "recipe", patch: { target: .1 } });
    s = reducer(s, { type: "validate" });
    s = reducer(s, { type: "recipe-create", name: " A 副本 ", copy: true });
    expect(s.recipe).toMatchObject({ name: "A 副本", candidate: 1, production: 0, target: .1 });
    expect(s.frames.every(f => f.imageId === null && !f.saved && !f.trial && !f.backup)).toBe(true);
    s = reducer(s, { type: "frame-param", key: "search", value: 8 });
    s = reducer(s, { type: "recipe-open", name: "工件 A · 壳体" });
    expect(s.recipe.target).toBe(.1); expect(s.frames).toEqual(original); expect(canPublish(s)).toBe(true);
    s = reducer(s, { type: "recipe-open", name: "A 副本" }); expect(s.frames[0].params.search).toBe(8);
    s = reducer(s, { type: "recipe-create", name: "新配方", copy: false });
    expect(s.recipe).toMatchObject({ candidate: 1, production: 0, target: 0 });
    expect(s.frames[0].params.search).toBe(4); expect(s.recipeLibrary).toHaveLength(4);
  });

  it("无效或重复名字不能创建，未知配方不能打开，最后一项不能删除", () => {
    const s = initialState();
    for (const name of [" ", "工件 B · 底板", "工件 A · 壳体"]) expect(reducer(s, { type: "recipe-create", name, copy: false })).toBe(s);
    expect(reducer(s, { type: "recipe-open", name: "missing" })).toBe(s);
    expect(reducer(s, { type: "recipe-delete", name: "missing" })).toBe(s);
    let next = reducer(s, { type: "recipe-delete", name: "工件 A · 壳体" });
    expect(next.recipe.name).toBe("工件 B · 底板"); expect(next.recipe.production).toBe(8);
    expect(reducer(next, { type: "recipe-delete", name: "工件 B · 底板" })).toBe(next);
  });

  it("运行中的配方和工位操作不会切换、删除或重标定本件", () => {
    const s = sceneState("live-running");
    expect(reducer(s, { type: "recipe-create", name: "new", copy: false })).toBe(s);
    expect(reducer(s, { type: "recipe-open", name: "工件 B · 底板" })).toBe(s);
    expect(reducer(s, { type: "recipe-delete", name: s.recipe.name })).toBe(s);
    expect(reducer(s, { type: "plc", patch: { ready: false } })).toBe(s);
    expect(reducer(s, { type: "calibration", patch: { saved: true } })).toBe(s);
    expect(reducer(s, { type: "live-option", patch: { scenario: "OK" } }).live.scenario).toBe("NG");
  });

  it("工位采集条件变化只使各工作台同设备样本失效，生产快照保留", () => {
    const s = sceneState("validation-pass");
    const next = reducer(s, { type: "device", patch: { gain: 8 } });
    expect(next.recipeLibrary.every(entry => entry.frames.filter(f => f.camera === "cam1").every(f => f.imageId === null && !f.saved && !f.trial) && entry.validation.status === "idle")).toBe(true);
    expect(next.recipeLibrary.map(entry => entry.frames.filter(f => f.camera !== "cam1"))).toEqual(s.recipeLibrary.map(entry => entry.frames.filter(f => f.camera !== "cam1")));
    expect(next.recipeLibrary.map(entry => entry.productionConfig)).toEqual(s.recipeLibrary.map(entry => entry.productionConfig));
    expect(s.frames[0].imageId).toBe(41);
  });

  it("重复保存本点或已保存的标定不反复增加候选修订", () => {
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

  it("schema 4 保留逐点资源与发布身份，旧预览存储重新开始", () => {
    const s = structuredClone(sceneState("validation-pass"));
    expect(restorePreview(s)).toEqual(s); expect(canPublish(restorePreview(s))).toBe(true);
    expect(restorePreview({ ...s, schema: 1 })).toEqual(initialState());
  });

  it("原包重现保留身份，候选复测不自动修复断胶，规则重判限制测量布局", () => {
    const s = initialState(), original = structuredClone(historyRecords);
    const replay = reducer(s, { type: "compare", kind: "remeasure", mode: "original" });
    expect(replay.comparisons[s.record]).toMatchObject({ mode: "original", verdict: "NG", gap: 6.2, version: 13, bundleId: historyRecords[0].bundleId, cycleId: historyRecords[0].cycleId });
    const changed = reducer(s, { type: "frame-path", path: [[150, 200], [800, 600]] });
    expect(reducer(changed, { type: "compare", kind: "rejudge" })).toBe(changed);
    const candidate = reducer(changed, { type: "compare", kind: "remeasure" });
    expect(candidate.comparisons[s.record]).toMatchObject({ mode: "candidate", verdict: "NG", gap: 6.2, version: 14 });
    expect(candidate.comparisons[s.record].bundleId).not.toBe(historyRecords[0].bundleId);
    const otherView = reducer(s, { type: "frame-plan", patch: { view: 2 } });
    expect(reducer(otherView, { type: "compare", kind: "remeasure" })).toBe(otherView);
    expect(reducer(otherView, { type: "capture", history: true })).toBe(otherView);
    expect(historyRecords).toEqual(original);
  });

  it.each(["device", "frame", "trial", "runtime", "phase", "comparison", "library"])("嵌套存储损坏 %s 恢复初始状态", kind => {
    const s = structuredClone(sceneState("validation-pass"));
    if (kind === "device") s.device = null as unknown as WorkflowState["device"];
    if (kind === "frame") s.frames[2].params = null as unknown as WorkflowState["frames"][number]["params"];
    if (kind === "trial") s.frames[2].trial!.score = NaN;
    if (kind === "runtime") s.productionConfig.shots = [];
    if (kind === "phase") s.live.phase = 6;
    if (kind === "comparison") s.comparisons["TJ-000184"] = null as unknown as WorkflowState["comparisons"][string];
    if (kind === "library") s.recipeLibrary.push(s.recipeLibrary[0]);
    expect(restorePreview(s)).toEqual(initialState());
  });
});
