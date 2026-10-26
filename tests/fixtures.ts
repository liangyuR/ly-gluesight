import { vi } from "vitest";
import type { DetectParams, Readiness, ReadinessItem, Recipe, RecipeDoc, RecipeOption, SampleSetSummary, SiteStatus, Snapshot } from "../src/features/cycle/types";
import type { PartDetail, PartSummary } from "../src/features/history/types";
import type { FrozenImage, WorkspaceView } from "../src/features/workspace/types";
import type { useWorkspace } from "../src/features/workspace/context";
import type {
  BurstJudgement, CalibrationView, CaptureRoundV2, FrameMetrics, FrameRecord, KindDefaults, PathSegment, PlanExport, ProgressV2,
  RecipeDocSegmentFields, RecipeSummaryV2, SegmentCalib, SegmentKind, WorkspacePlanView,
} from "../src/features/plan/types";

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

export function workspaceState(view = burstWorkspaceView()): ReturnType<typeof useWorkspace> {
  return {
    list: [burstSummary(view.workspace.doc as BurstRecipeDoc)], drafts: [view.workspace], cameras: [], unavailable: [], selectedId: view.workspace.doc.id,
    data: view, doc: view.workspace.doc, preview: view.layout, previewError: "", error: "",
    saveState: "saved", retrySave: vi.fn(), busy: false, dirty: false, frameDirty: false, corridorDrafts: {},
    select: vi.fn(async () => view), clearSelection: vi.fn(), reloadList: vi.fn(async () => {}),
    setDoc: vi.fn(), setCorridorDraft: vi.fn(), setError: vi.fn(),
    saveDoc: vi.fn(async () => view), act: vi.fn(async request => request()),
  };
}

export function snapshot(phase: Snapshot["phase"] = "IDLE"): Snapshot {
  return {
    measurementWorkers: { capacity: 2, running: 0, timedOut: 0, availableCapacity: 2 },
    phase, since: 1, fault: null, productSource: "manual", activeRecipeId: "A", triggerMode: "fly",
    part: null, result: null, stats: { total: 0, ok: 0, ng: 0, err: 0 }, strayFrames: 0, alarms: [],
  };
}

export function siteStatus(over: Partial<SiteStatus> = {}): SiteStatus {
  return { mode: "real", source: null, last: null, measurement: "lyFlow", port: null, running: false, continuous: false, parts: 0, message: "",
    heartbeatStopped: false, sampleSet: null, nextPart: null, recent: [], ...over };
}

export function sampleSet(name = "壳体样本"): SampleSetSummary {
  return { id: "set-1", name, source: "现场", dir: "C:/samples/set-1", shotCount: 2, cameras: [{ id: "CAM-1", views: [1] }], mmPerPx: 0.1,
    parts: [{ partId: "part-1", sn: "SN1", label: "ok" }], missing: [], missingCount: 0 };
}

export function recipeOption(id: string, runnable = true, over: Partial<RecipeOption> = {}): RecipeOption {
  return { id, name: `工件 ${id}`, version: 1, productCode: 1, published: true, runnable,
    reason: runnable ? null : "与样本集不符", target: runnable ? null : "workspace", waiting: null, ...over };
}

export function readiness(over: Partial<Readiness> = {}): Readiness {
  const items: ReadinessItem[] = (["plc", "camera", "recipe", "engine", "audit", "site"] as const).map(key => ({ key, title: key, state: "ok", detail: "就绪", hint: null, target: null }));
  return { ready: true, items, recipes: [recipeOption("A"), recipeOption("B")], ...over };
}

// ───────────── 段模型（分段连拍飞拍）共用夹具 ─────────────
// 场景：三台相机共用一个触发；段 直线 120 mm → 弯道 30 mm → 直线 80 mm，各段 N_exp 待标定（分段算法未就绪），
// 整件凭 10 轮标定得 ΣN_exp = 12、Σtol = 3；一件 12 个帧组，CAM-2 在 seq 5 缺帧。

export type BurstRecipeDoc = RecipeDoc & RecipeDocSegmentFields;

/** 段配方的计划相机（有序）。 */
export const burstCameras = ["CAM-1", "CAM-2", "CAM-3"];
/** 一件的帧组数；CAM-2（下标 1）缺 seq 5 */
export const burstGroups = 12;
export const burstMissing = { camera: 1, seq: 5 };
const partStats: SegmentCalib = { rounds: 10, min: 11, max: 13, mean: 11.9 };
const roundGroups = [12, 12, 11, 12, 13, 12, 12, 12, 11, 12];
/** 各相机帧数：缺帧的相机少一帧 */
export const burstCameraFrames = () => burstCameras.map((_, c) => burstGroups - (c === burstMissing.camera ? 1 : 0));

/** 与后端 KindDefaults::default 相同：容差都为 1；胶宽 4 ± 1.5 mm、帧内允许超差 5 mm、断胶 6 mm、有胶比例 0.8、溢胶 8 mm；直线段胶偏 ±2 mm。 */
export function kindDefaults(): KindDefaults {
  const checks = (straight: boolean) => ({
    width: { enabled: true, nominalMm: 4, tolUpper: 1.5, tolLower: 1.5, maxExcursionMm: 5, maxRangeMm: null },
    break: { enabled: true, maxGapMm: 6, minPresent: .8 },
    overflow: { enabled: true, maxWidthMm: 8 },
    offset: straight ? { enabled: true, maxMeanShiftMm: 2, maxLocalDevMm: 2 } : null,
  });
  return { straight: { tolerance: 1, checks: checks(true) }, corner: { tolerance: 1, checks: checks(false) } };
}

/** 新建一段（同后端 new_segment）：容差与检测项取该类型默认值，N_exp、标定、胶偏参考为空。 */
export function pathSegment(index: number, kind: SegmentKind, over: Partial<PathSegment> = {}): PathSegment {
  const d = kindDefaults()[kind];
  return { index, kind, lengthMm: null, expectedFrames: null, tolerance: d.tolerance, calib: null, checks: d.checks, offsetRef: null, inspectRange: null, ...over };
}

/** 后端口径：各段 N_exp 都有时为其和，否则取整件标定；容差为各段之和。 */
export function burstTotals(doc: Pick<RecipeDocSegmentFields, "segments" | "partCalib">) {
  const frames = doc.segments.map(s => s.expectedFrames);
  const expectedTotal = frames.length && frames.every(n => n != null) ? frames.reduce<number>((sum, n) => sum + n!, 0) : doc.partCalib?.expectedTotal ?? null;
  return { expectedTotal, toleranceTotal: doc.segments.reduce((sum, s) => sum + s.tolerance, 0) };
}

/** 三台相机、三段的段配方（schema 8）：走廊已示教，各段 N_exp 待标定，整件已标定（可发布）。新建未标定的配方传 { partCalib: null }。 */
export function burstRecipe(over: Partial<BurstRecipeDoc> = {}): BurstRecipeDoc {
  const detect: DetectParams = { searchMm: 4, polarity: "dark", widthRange: [1, 6] };
  return {
    id: "S", name: "段配方 S", version: 1, planVersion: 3, productCode: 2, triggerMode: "fly", schemaVersion: 8,
    spacing: 1, filterWindow: 3, detect, limits: { position: null, width: null, maxGapLen: 6, minPresent: .8 },
    cameras: Object.fromEntries(burstCameras.map(c => [c, { exposureUs: 60, gainDb: 6 }])),
    segments: [
      pathSegment(1, "straight", { lengthMm: 120, inspectRange: { aMm: 5, bMm: 115 }, offsetRef: { measure: "absoluteX", camera: "CAM-1", xRef: null, spread: null } }),
      pathSegment(2, "corner", { lengthMm: 30 }),
      pathSegment(3, "straight", { lengthMm: 80 }),
    ],
    inspect: burstCameras.map(camera => ({ camera, corridor: [[10, 30], [90, 30]], mmPerPx: .1, calib: null, detect: null })),
    segmentDefaults: kindDefaults(), calibrationRounds: 10,
    partCalib: { expectedTotal: 12, calib: { ...partStats } },
    ...over,
  };
}

export function burstSummary(doc = burstRecipe()): RecipeSummaryV2 {
  const { expectedTotal, toleranceTotal } = burstTotals(doc);
  return { id: doc.id, name: doc.name, version: doc.version, revisionId: `revisionId-${doc.id}`, productCode: doc.productCode, triggerMode: "fly",
    cameras: doc.inspect.map(i => i.camera), length: doc.segments.reduce((sum, s) => sum + (s.lengthMm ?? 0), 0), planVersion: doc.planVersion ?? 0, triggerCount: expectedTotal,
    segmentCount: doc.segments.length, expectedTotal, toleranceTotal, calibrated: expectedTotal != null };
}

export function planExport(doc = burstRecipe()): PlanExport {
  return { protocolVersion: 7, recipeId: doc.id, productCode: doc.productCode, planVersion: doc.planVersion ?? 0, cameras: doc.inspect.map(i => i.camera),
    segments: doc.segments.map(({ index, kind, lengthMm, expectedFrames, tolerance, inspectRange }) => ({ index, kind, lengthMm, expectedFrames, tolerance, inspectRange })),
    ...burstTotals(doc) };
}

/** 段配方进度的静态样例：采集、段列表完成，走廊 1 / 3 台，尚未标定。 */
export function progressV2(): ProgressV2 {
  const step = (id: ProgressV2["steps"][number]["id"], label: string, status: ProgressV2["steps"][number]["status"], detail: string) => ({ id, label, optional: false, status, detail, blockers: [] });
  return {
    steps: [
      step("capture", "整圈采集", "done", "已有完整采集轮次"),
      step("segments", "段列表", "done", "3 段"),
      step("corridor", "走廊示教", "inProgress", "1 / 3 台相机已示教"),
      step("calibration", "多轮标定", "notStarted", "0 / 10 轮"),
      step("validation", "验证", "notStarted", "待标定"),
      step("publish", "发布与生效", "notStarted", "未发布"),
    ],
    next: { step: "corridor", label: "示教 CAM-2 的走廊", target: { page: "corridor", camera: "CAM-2" } },
  };
}

/** 多轮标定结果：10 轮整件帧组数 → ΣN_exp 12（min 11、max 13），各段待分段算法。 */
export function calibrationView(): CalibrationView {
  return {
    minRounds: 10,
    rounds: roundGroups.map((groupCount, i) => ({ roundId: `round-${i + 1}`, standard: false, groupCount, cameraFrames: burstCameras.map(() => groupCount), incompleteGroups: 0, lossAlarms: 0, createdAt: i + 1 })),
    part: { nExp: 12, calib: { ...partStats }, suggestedTolerance: 1 },
    segments: burstRecipe().segments.map(s => ({ index: s.index, kind: s.kind, outcome: null, pending: "分段算法未就绪", tolerance: s.tolerance, xRef: null, xRefPending: s.kind === "straight" ? "分段算法未就绪" : null })),
    expectedTotal: 12, toleranceTotal: 3, planVersionChange: false,
  };
}

/** 新示教采集轮次：12 个帧组，CAM-2 缺 seq 5（只告警，轮次照常完成）。 */
export function captureRoundV2(over: Partial<CaptureRoundV2> = {}): CaptureRoundV2 {
  const cameraFrames = burstCameraFrames();
  return {
    format: 2, roundId: "round-1", recipeId: "S", state: "complete", simulated: false,
    cameras: burstCameras.map((cameraId, c) => ({ cameraId, deviceSession: 1, receivedCount: cameraFrames[c] })),
    plcShotCount: 12, plcPlanVersion: 3, deviceTriggers: 12, groupCount: burstGroups, cameraFrames,
    incompleteGroups: [burstMissing.seq], missingFrames: [{ ...burstMissing }], lossAlarms: [], alarms: [],
    createdAt: 1, endedAt: 2, error: null, failure: null, ...over,
  };
}

function burstFrozen(camera: string): FrozenImage {
  return { id: `S-${camera}-frame`, source: "capture", capturedAt: 1, size: [100, 60], camera, view: 1,
    cameraTag: { id: camera, source: "sim", viewCount: 1, acquisition: "triggered" }, calibTag: { path: null, value: null }, geometryTag: [camera], exposureUs: 60, gainDb: 6, historyId: null };
}

/** 段配方的候选工作区（WorkspaceView.plan）：CAM-1 已示教走廊，CAM-2 已选帧待示教，CAM-3 未选帧。 */
export function burstWorkspaceView(doc = burstRecipe()): WorkspaceView {
  const totals = burstTotals(doc);
  const plan: WorkspacePlanView = {
    segments: doc.segments, cameras: doc.inspect.map(i => i.camera), segmentDefaults: doc.segmentDefaults, calibrationRounds: doc.calibrationRounds, partCalib: doc.partCalib,
    ...totals, calibrated: totals.expectedTotal != null,
    cameraTeaching: [
      { camera: "CAM-1", saved: true, frame: burstFrozen("CAM-1"), mmPerPx: .1 },
      { camera: "CAM-2", saved: false, frame: burstFrozen("CAM-2"), mmPerPx: null },
      { camera: "CAM-3", saved: false, frame: null, mmPerPx: null },
    ],
    calibration: calibrationView(), productionExport: null, candidateExport: planExport(doc), candidateExportError: null, shotCountChanged: false, progress: progressV2(),
  };
  const layout: Recipe = { ...structuredClone(doc), planVersion: doc.planVersion ?? 0, revisionId: `revisionId-${doc.id}`, pathSegments: structuredClone(doc.segments) };
  const cameraTeaching = plan.cameraTeaching.flatMap(({ camera, frame }) => frame ? [{ camera, frame, round: "round-1", seq: 1 }] : []);
  const rounds = roundGroups.map((groupCount, i) => ({ roundId: `round-${i + 1}`, standard: false, createdAt: i + 1, cameras: plan.cameras, cameraConfig: plan.cameras.map(() => null),
    groupCount, cameraFrames: plan.cameras.map(() => groupCount), incompleteGroups: 0, lossAlarms: 0, corridors: plan.cameras.map(() => null), xMeans: plan.cameras.map(() => null) }));
  return {
    layout, productionVersion: null, productionPlanVersion: null, planChanged: false, plan,
    workspace: { doc, baseRevision: null, revision: 3, updatedAt: 1, publish: { state: "draft" }, pending: null,
      samples: [], sampleBank: [], validation: null, cameraTeaching, calibration: { rounds } },
  };
}

/** 一帧的独立指标（measured）。 */
export function frameMetrics(camera: number, seq: number, over: Partial<FrameMetrics> = {}): FrameMetrics {
  return { camera, seq, state: "measured", error: null, widthMin: 3.8, widthMax: 4.2, widthMean: 4, widths: [3.8, 4, 4.2], spacingMm: 1, gapMaxMm: 0, presentRatio: 1, xMm: .1, ...over };
}

/** 段配方一件的判定：整条胶路一个未分段区域，OK；CAM-2 缺一帧 → 检测不完整，胶偏未判，气泡、拉丝未检。 */
export function burstJudgement(over: Partial<BurstJudgement> = {}): BurstJudgement {
  return {
    verdict: "OK", plcCode: 1, faultCode: 0, reason: "合格", groupCount: burstGroups, countStatus: "normal",
    regions: [{
      region: { kind: { type: "unsegmented", firstSegment: 1, lastSegment: 3, reason: "algorithmPending" }, firstSeq: 1, lastSeq: burstGroups },
      verdict: "OK", reason: "", frameCount: burstGroups, expected: 12, tolerance: 3, countStatus: "normal", cameraFrames: burstCameraFrames(),
      width: { min: 3.8, max: 4.2, mean: 4 }, offset: null, offsetUnjudged: "unsegmented", incompleteGroups: [burstMissing.seq],
    }],
    flags: { incomplete: true, frameCountAnomaly: false, offsetUnjudged: true, unsegmented: true, shortPart: false },
    alarms: [], uninspected: [{ segment: 1, fromMm: 0, toMm: 5 }, { segment: 1, fromMm: 115, toMm: 120 }], unchecked: ["bubble", "stringing"],
    ...over,
  };
}

export function burstPartSummary(id = 1): PartSummary {
  return { id, sn: `SN-${100 + id}`, ts: 1, recipeId: "S", recipeVersion: 1, recipeRevision: "revisionId-S", triggerMode: "fly",
    verdict: "OK", plcCode: 1, faultCode: 0, reason: "合格", drainMs: 20, framesExpected: 12, framesReceived: burstGroups,
    retestOf: null, cycleId: `cycle-${id}`, bundleId: "bundle-S", delivery: { state: "acknowledged", updatedAt: 1, message: null },
    source: "real", sample: null, flags: burstJudgement().flags };
}

/** 段配方一件的历史详情：每台相机每个 seq 一条帧记录（缺帧也有）。 */
export function burstPartDetail(): PartDetail {
  const judgement = burstJudgement();
  const frames: FrameRecord[] = burstCameras.flatMap((camera, c) => Array.from({ length: burstGroups }, (_, i): FrameRecord => {
    const seq = i + 1;
    return c === burstMissing.camera && seq === burstMissing.seq
      ? { camera: c, seq, status: "missing", frameCounter: null, triggerCounter: null, region: 0, metrics: null, rawFile: null }
      : { camera: c, seq, status: "measured", frameCounter: 1000 * (c + 1) + seq, triggerCounter: seq, region: 0, metrics: frameMetrics(c, seq), rawFile: `frames/${camera}/${String(seq).padStart(5, "0")}.pgm` };
  }));
  return {
    summary: burstPartSummary(), judgement: { verdict: judgement.verdict, plcCode: judgement.plcCode, faultCode: judgement.faultCode, reason: judgement.reason, segments: [], gaps: [] },
    frames: [], shots: [], points: null, triggers: 12, softwareVersion: "test", retests: [],
    recording: { state: "complete", available: true, directory: "recorded", errors: [] },
    burst: { segments: burstRecipe().segments, cameras: burstCameras, expectedTotal: 12, toleranceTotal: 3, deviceTriggers: 12, cameraFrames: burstCameraFrames(), judgement, frames },
  };
}
