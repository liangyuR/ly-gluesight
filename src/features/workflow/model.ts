export type View = "guide" | "device" | "plc" | "calibration" | "recipes" | "geometry" | "teach" | "overview" | "validation" | "live" | "history" | "record" | "settings";
export type Verdict = "OK" | "NG" | "ERR";
export type Trial = { imageId: number; revision: number; pass: boolean; score: number };
export interface RecipeConfiguration {
  name: string; candidate: number; production: number; revision: number;
  width: number; height: number; radius: number; fovWidth: number; fovHeight: number; speed: number;
  target: number; tolerance: number; maxGap: number;
}
export interface RuntimeConfiguration {
  version: number;
  recipe: RecipeConfiguration;
  frameParams: TeachFrame["params"][];
  calibrationVersion: string;
  overview: { positions: { x: number; y: number }[]; background: string | null; saved: boolean };
}
export interface TeachFrame {
  id: number;
  imageId: number | null;
  source: "camera" | "history";
  sourceRecord: string | null;
  quality: "normal" | "low";
  captureSettings: { exposure: number; calibrationVersion: string };
  revision: number;
  trial: Trial | null;
  saved: boolean;
  backup: Pick<TeachFrame, "imageId" | "source" | "sourceRecord" | "quality" | "captureSettings" | "params"> | null;
  params: { search: number; contrast: number; minWidth: number; maxWidth: number };
}
export interface WorkflowState {
  schema: 1;
  scene: string;
  device: { connected: boolean; applied: boolean; exposure: number; gain: number; trigger: string };
  plc: { connected: boolean; ready: boolean; address: string; protocol: string; points: string[]; pointsApplied: boolean };
  calibration: { captured: boolean; result: "idle" | "pass" | "fail"; saved: boolean; sample: "good" | "bad"; version: number };
  recipe: RecipeConfiguration;
  recipeLibrary: RecipeWorkspace[];
  productionConfig: RuntimeConfiguration;
  frames: TeachFrame[];
  selectedFrame: number;
  imageSequence: number;
  overview: { positions: { x: number; y: number }[]; background: string | null; saved: boolean };
  validation: { status: "idle" | "passed" | "failed"; revision: number | null };
  live: { accepting: boolean; phase: number; auto: boolean; continuous: boolean; scenario: Verdict; result: Verdict | null; inFlightVersion: number | null; inFlightConfig: RuntimeConfiguration | null; queued: number | null; queuedConfig: RuntimeConfiguration | null; part: number };
  record: string;
  comparisons: Record<string, { kind: "remeasure" | "rejudge"; verdict: Verdict; gap: number; version: number }>;
  settings: { retention: number; timeout: number; raw: string; saved: boolean };
}
export type RecipeWorkspace = Pick<WorkflowState, "recipe" | "productionConfig" | "frames" | "overview" | "validation">;
export const plcPoints = (protocol: string) => protocol === "Modbus TCP" ? ["00001", "00002", "00003", "00004", "40001", "00005"] : ["DB20.DBX0.0", "DB20.DBX0.1", "DB20.DBX0.2", "DB20.DBX0.3", "DB20.DBW2", "DB20.DBX4.0"];
export const initialPositions = [{ x: 0.18, y: 0.21 }, { x: 0.5, y: 0.21 }, { x: 0.82, y: 0.21 }, { x: 0.82, y: 0.79 }, { x: 0.5, y: 0.79 }, { x: 0.18, y: 0.79 }];
export const defaultRecipe: RecipeConfiguration = { name: "工件 A · 壳体", candidate: 14, production: 13, revision: 1, width: 520, height: 230, radius: 28, fovWidth: 216, fovHeight: 145, speed: 300, target: 3, tolerance: 1, maxGap: 0.5 };
function runtimeConfig(recipe: RecipeConfiguration, frames: TeachFrame[], overview: WorkflowState["overview"], calibrationVersion: string, version: number): RuntimeConfiguration {
  return { version, recipe: { ...recipe }, frameParams: frames.map(f => ({ ...f.params })), calibrationVersion, overview: { ...overview, positions: overview.positions.map(p => ({ ...p })) } };
}

export function initialState(): WorkflowState {
  const recipe = { ...defaultRecipe };
  const frames: TeachFrame[] = Array.from({ length: 6 }, (_, i) => ({ id: i + 1, imageId: 41 + i, source: "camera", sourceRecord: null, quality: "normal", captureSettings: { exposure: 60, calibrationVersion: "C07" }, revision: 1, trial: i === 2 ? null : { imageId: 41 + i, revision: 1, pass: true, score: 0.94 }, saved: i !== 2, backup: null, params: { search: 4, contrast: 32, minWidth: 2.5, maxWidth: 5 } }));
  const overview = { positions: initialPositions.map(p => ({ ...p })), background: null, saved: false };
  const workspace: RecipeWorkspace = { recipe, frames, productionConfig: runtimeConfig(recipe, frames, overview, "C07", 13), overview, validation: { status: "idle", revision: null } };
  const other = (name: string, production: number): RecipeWorkspace => {
    const r = { ...recipe, name, candidate: production + 1, production };
    const otherFrames = structuredClone(frames), otherOverview = structuredClone(overview);
    return { ...workspace, frames: otherFrames, overview: otherOverview, recipe: r, productionConfig: runtimeConfig(r, otherFrames, otherOverview, "C07", production) };
  };
  return {
    schema: 1, scene: "default",
    device: { connected: true, applied: true, exposure: 60, gain: 6, trigger: "Line0 · 上升沿" },
    plc: { connected: true, ready: true, address: "192.168.1.10", protocol: "S7", points: plcPoints("S7"), pointsApplied: true },
    calibration: { captured: true, result: "pass", saved: true, sample: "good", version: 7 },
    ...workspace, recipeLibrary: [workspace, other("工件 B · 底板", 8)],
    selectedFrame: 3, imageSequence: 50,
    live: { accepting: false, phase: 0, auto: false, continuous: false, scenario: "NG", result: null, inFlightVersion: null, inFlightConfig: null, queued: null, queuedConfig: null, part: 184 },
    record: "TJ-000184", comparisons: {},
    settings: { retention: 30, timeout: 3000, raw: "NG 与 ERR", saved: true },
  };
}

export function roundedPath(width: number, height: number, radius: number): { x: number; y: number }[] {
  const r = Math.max(0, Math.min(radius, width / 2, height / 2));
  const points: { x: number; y: number }[] = [];
  const addLine = (x1: number, y1: number, x2: number, y2: number) => {
    const n = Math.max(2, Math.ceil(Math.hypot(x2 - x1, y2 - y1) / 3));
    for (let i = 0; i < n; i++) points.push({ x: x1 + (x2 - x1) * i / n, y: y1 + (y2 - y1) * i / n });
  };
  const addArc = (cx: number, cy: number, a: number) => {
    for (let i = 0; i < 16; i++) points.push({ x: cx + r * Math.cos((a + i * 90 / 16) * Math.PI / 180), y: cy + r * Math.sin((a + i * 90 / 16) * Math.PI / 180) });
  };
  addLine(r, 0, width - r, 0); addArc(width - r, r, -90);
  addLine(width, r, width, height - r); addArc(width - r, height - r, 0);
  addLine(width - r, height, r, height); addArc(r, height - r, 90);
  addLine(0, height - r, 0, r); addArc(r, r, 180);
  return points;
}
export function coverage(state: WorkflowState): number {
  const g = state.recipe;
  if (![g.width, g.height, g.fovWidth, g.fovHeight, g.radius].every(Number.isFinite) || g.width < 100 || g.width > 2000 || g.height < 100 || g.height > 1200 || g.fovWidth < 50 || g.fovWidth > 1500 || g.fovHeight < 50 || g.fovHeight > 1500 || g.radius < 0 || g.radius > Math.min(g.width, g.height) / 2) return 0;
  const points = roundedPath(g.width, g.height, g.radius);
  const covered = points.filter(p => initialPositions.some((c, i) => {
    const margin = state.frames[i].params.search;
    return Math.abs(p.x - c.x * g.width) + margin <= g.fovWidth / 2 && Math.abs(p.y - c.y * g.height) + margin <= g.fovHeight / 2;
  })).length;
  return Math.round(covered / points.length * 1000) / 10;
}
export function canSaveFrame(f: TeachFrame): boolean {
  return f.imageId !== null && f.trial?.pass === true && f.trial.imageId === f.imageId && f.trial.revision === f.revision;
}
export function validationChecks(s: WorkflowState): { label: string; pass: boolean; view: View; detail: string }[] {
  return [
    { label: "设备与握手就绪", pass: s.device.connected && s.device.applied && s.plc.connected && s.plc.ready, view: "device", detail: "相机接受参数，PLC 业务握手就绪" },
    { label: "工位标定有效", pass: s.calibration.saved, view: "calibration", detail: "标定 C" + String(s.calibration.version).padStart(2, "0") + (s.calibration.saved ? " 已保存" : " 待确认") },
    { label: "胶路与搜索窗口覆盖", pass: coverage(s) === 100, view: "geometry", detail: String(coverage(s)) + "% · 包含搜索余量" },
    { label: "所有帧示教已保存", pass: s.frames.every(f => f.saved && canSaveFrame(f)), view: "teach", detail: s.frames.filter(f => f.saved && canSaveFrame(f)).length + " / 6 帧" },
  ];
}
export function canPublish(s: WorkflowState): boolean {
  return s.validation.status === "passed" && s.validation.revision === s.recipe.revision && validationChecks(s).every(c => c.pass) && s.recipe.candidate !== s.recipe.production && s.live.queued === null;
}
export const validationSamples = [
  { id: "S01", name: "正常胶路", expected: "OK" as Verdict, d: 3.3, width: 3.94, gap: 0, valid: true },
  { id: "S02", name: "断胶样本", expected: "NG" as Verdict, d: 3.2, width: 3.8, gap: 6.2, valid: true },
  { id: "S03", name: "位置偏离", expected: "NG" as Verdict, d: 5.2, width: 3.9, gap: 0, valid: true },
  { id: "S04", name: "胶宽超限", expected: "NG" as Verdict, d: 3.2, width: 5.8, gap: 0, valid: true },
  { id: "S05", name: "定位失败", expected: "ERR" as Verdict, d: 0, width: 0, gap: 0, valid: false },
];
export function sampleVerdict(s: WorkflowState, sample: typeof validationSamples[number]): Verdict {
  if (!sample.valid) return "ERR";
  const p = s.frames[2].params;
  if (Math.abs(sample.d - s.recipe.target) > s.recipe.tolerance || sample.width < p.minWidth || sample.width > p.maxWidth || sample.gap > s.recipe.maxGap) return "NG";
  return "OK";
}
export const historyRecords = [
  { id: "TJ-000184", time: "2026-10-08 14:32:08", result: "NG" as Verdict, cause: "断胶 · 6.2 mm", raw: true, frames: 6, version: 13, gap: 6.2 },
  { id: "TJ-000183", time: "2026-10-08 14:31:52", result: "OK" as Verdict, cause: "胶路合格", raw: true, frames: 6, version: 13, gap: 0 },
  { id: "TJ-000182", time: "2026-10-07 09:22:14", result: "OK" as Verdict, cause: "仅测量数据", raw: false, frames: 6, version: 13, gap: 0 },
  { id: "TJ-000181", time: "2026-10-07 09:21:57", result: "ERR" as Verdict, cause: "k3 原图缺失", raw: true, frames: 5, version: 13, gap: 0 },
];

type DevicePatch = Partial<WorkflowState["device"]>;
type PlcPatch = Partial<WorkflowState["plc"]>;
type RecipePatch = Partial<WorkflowState["recipe"]>;
export type Action =
  | { type: "load"; state: WorkflowState }
  | { type: "device"; patch: DevicePatch }
  | { type: "plc"; patch: PlcPatch }
  | { type: "calibration"; patch: Partial<WorkflowState["calibration"]> }
  | { type: "recipe"; patch: RecipePatch; geometry?: boolean }
  | { type: "recipe-create"; name: string; copy: boolean }
  | { type: "recipe-open"; name: string }
  | { type: "recipe-delete"; name: string }
  | { type: "select-frame"; id: number }
  | { type: "capture"; quality?: "normal" | "low"; history?: boolean }
  | { type: "frame-param"; key: keyof TeachFrame["params"]; value: number }
  | { type: "trial" } | { type: "save-frame" } | { type: "restore-frame" }
  | { type: "overview"; patch: Partial<WorkflowState["overview"]> }
  | { type: "validate" } | { type: "publish" }
  | { type: "live-start" } | { type: "live-next" } | { type: "live-stop" } | { type: "live-step" }
  | { type: "live-option"; patch: Partial<WorkflowState["live"]> }
  | { type: "record"; id: string }
  | { type: "compare"; kind: "remeasure" | "rejudge" }
  | { type: "settings"; patch: Partial<WorkflowState["settings"]> };

function changed(s: WorkflowState): WorkflowState {
  return { ...s, recipe: { ...s.recipe, revision: s.recipe.revision + 1, candidate: Math.max(s.recipe.candidate, s.recipe.production + 1, (s.live.queued ?? 0) + 1) }, validation: { status: "idle", revision: null } };
}
const working = (s: WorkflowState) => s.live.phase > 0 && s.live.phase < 4;
function workspaceOf(s: WorkflowState): RecipeWorkspace {
  return { recipe: s.recipe, productionConfig: s.productionConfig, frames: s.frames, overview: s.overview, validation: s.validation };
}
export function reducer(s: WorkflowState, a: Action): WorkflowState {
  const next = reduce(s, a);
  if (next === s) return s;
  const workspace = workspaceOf(next);
  const recapture = a.type === "device" && ("exposure" in a.patch || "gain" in a.patch || "trigger" in a.patch) || a.type === "calibration" && a.patch.saved;
  return { ...next, recipeLibrary: next.recipeLibrary.map(entry => {
    if (entry.recipe.name === next.recipe.name) return workspace;
    if (!recapture) return entry;
    return { ...entry, recipe: { ...entry.recipe, revision: entry.recipe.revision + 1, candidate: Math.max(entry.recipe.candidate, entry.recipe.production + 1) }, validation: { status: "idle", revision: null }, frames: entry.frames.map(f => ({ ...f, imageId: null, trial: null, saved: false, revision: f.revision + 1 })) };
  }) };
}
function reduce(s: WorkflowState, a: Action): WorkflowState {
  switch (a.type) {
    case "load": return a.state;
    case "device": {
      if (s.live.phase > 0 && s.live.phase < 4) return s;
      const edit = "exposure" in a.patch || "gain" in a.patch || "trigger" in a.patch;
      if (!edit) return { ...s, device: { ...s.device, ...a.patch } };
      return changed({ ...s, device: { ...s.device, ...a.patch, applied: false }, frames: s.frames.map(f => ({ ...f, imageId: null, trial: null, saved: false })) });
    }
    case "plc": return working(s) ? s : { ...s, plc: { ...s.plc, ...a.patch } };
    case "calibration": {
      if (working(s) || (a.patch.saved && (s.calibration.saved || s.calibration.result !== "pass"))) return s;
      const next = { ...s, calibration: { ...s.calibration, ...a.patch, version: a.patch.saved && !s.calibration.saved ? s.calibration.version + 1 : s.calibration.version } };
      return a.patch.saved ? changed({ ...next, frames: next.frames.map(f => ({ ...f, imageId: null, trial: null, saved: false, revision: f.revision + 1 })) }) : next;
    }
    case "recipe": {
      if (s.live.phase > 0 && s.live.phase < 4 && s.recipe.candidate === s.recipe.production && a.patch.candidate === undefined) return s;
      const next = changed({ ...s, recipe: { ...s.recipe, ...a.patch } });
      const config = a.patch.production !== undefined ? runtimeConfig(next.recipe, next.frames, next.overview, "C07", a.patch.production) : next.productionConfig;
      return { ...next, productionConfig: config, frames: a.geometry ? next.frames.map(f => ({ ...f, imageId: null, trial: null, saved: false, revision: f.revision + 1 })) : next.frames };
    }
    case "recipe-create": {
      const name = a.name.trim();
      if (!name || working(s) || s.live.queued !== null || s.recipeLibrary.some(entry => entry.recipe.name === name)) return s;
      const recipe = { ...(a.copy ? s.recipe : defaultRecipe), name, candidate: 1, production: 0, revision: 1 };
      const frames = s.frames.map(f => ({ ...f, imageId: null, source: "camera" as const, sourceRecord: null, quality: "normal" as const, revision: 1, trial: null, saved: false, backup: null, params: { ...(a.copy ? f.params : { search: 4, contrast: 32, minWidth: 2.5, maxWidth: 5 }) } }));
      const overview = { positions: initialPositions.map(p => ({ ...p })), background: null, saved: false };
      const workspace: RecipeWorkspace = { recipe, frames, overview, validation: { status: "idle", revision: null }, productionConfig: runtimeConfig(recipe, frames, overview, "C" + String(s.calibration.version).padStart(2, "0"), 0) };
      return { ...s, ...workspace, selectedFrame: 1, recipeLibrary: [...s.recipeLibrary, workspace] };
    }
    case "recipe-open": {
      const entry = s.recipeLibrary.find(entry => entry.recipe.name === a.name);
      if (!entry || (working(s) && a.name !== s.recipe.name) || s.live.queued !== null) return s;
      const next = { ...s, ...entry, selectedFrame: 1 };
      return next.recipe.candidate === next.recipe.production ? changed(next) : next;
    }
    case "recipe-delete": {
      if (working(s) || s.live.queued !== null || s.recipeLibrary.length <= 1) return s;
      const recipeLibrary = s.recipeLibrary.filter(entry => entry.recipe.name !== a.name);
      if (recipeLibrary.length === s.recipeLibrary.length) return s;
      return { ...s, ...(a.name === s.recipe.name ? recipeLibrary[0] : {}), recipeLibrary, selectedFrame: a.name === s.recipe.name ? 1 : s.selectedFrame };
    }
    case "select-frame": return Number.isInteger(a.id) && a.id >= 1 && a.id <= 6 ? { ...s, selectedFrame: a.id } : s;
    case "capture": {
      const record = a.history ? historyRecords.find(r => r.id === s.record) : null;
      if (a.history ? !record?.raw || record.result === "ERR" && s.selectedFrame === 3 : !s.device.connected || !s.device.applied) return s;
      return changed({
      ...s, imageSequence: s.imageSequence + 1,
      frames: s.frames.map(f => f.id === s.selectedFrame ? { ...f, imageId: a.history ? Number(s.record.replace("TJ-", "")) * 1000 + f.id : s.imageSequence + 1, source: a.history ? "history" : "camera", sourceRecord: a.history ? s.record : null, quality: a.quality ?? "normal", captureSettings: { exposure: a.history ? 60 : s.device.exposure, calibrationVersion: a.history ? "C07" : "C" + String(s.calibration.version).padStart(2, "0") }, trial: null, saved: false, backup: a.history ? (f.backup ?? { imageId: f.imageId, source: f.source, sourceRecord: f.sourceRecord, quality: f.quality, captureSettings: { ...f.captureSettings }, params: { ...f.params } }) : f.backup } : f),
      });
    }
    case "frame-param": return changed({ ...s, frames: s.frames.map(f => f.id === s.selectedFrame ? { ...f, params: { ...f.params, [a.key]: a.value }, revision: f.revision + 1, trial: null, saved: false } : f) });
    case "trial": return { ...s, frames: s.frames.map(f => {
      if (f.id !== s.selectedFrame || f.imageId === null) return f;
      const pass = (f.quality === "normal" ? 80 : 14) >= f.params.contrast && f.params.contrast > 0 && f.params.search >= 1.5 && f.params.minWidth > 0 && f.params.minWidth < f.params.maxWidth;
      return { ...f, saved: false, trial: { imageId: f.imageId, revision: f.revision, pass, score: pass ? f.quality === "normal" ? 0.94 : 0.82 : 0.31 } };
    }) };
    case "save-frame": return canSaveFrame(s.frames[s.selectedFrame - 1]) && !s.frames[s.selectedFrame - 1].saved ? changed({ ...s, frames: s.frames.map(f => f.id === s.selectedFrame ? { ...f, saved: true } : f) }) : s;
    case "restore-frame": return changed({ ...s, frames: s.frames.map(f => f.id === s.selectedFrame ? { ...f, ...(f.backup ?? { params: { search: 4, contrast: 32, minWidth: 2.5, maxWidth: 5 } }), backup: null, trial: null, saved: false, revision: f.revision + 1 } : f) });
    case "overview": return { ...s, overview: { ...s.overview, ...a.patch } };
    case "validate": {
      if (!validationChecks(s).every(c => c.pass)) return s;
      const pass = validationSamples.every(sample => sampleVerdict(s, sample) === sample.expected);
      return { ...s, validation: { status: pass ? "passed" : "failed", revision: s.recipe.revision } };
    }
    case "publish": {
      if (!canPublish(s)) return s;
      const config = runtimeConfig(s.recipe, s.frames, s.overview, "C" + String(s.calibration.version).padStart(2, "0"), s.recipe.candidate);
      if (s.live.phase > 0 && s.live.phase < 4) return { ...s, live: { ...s.live, queued: s.recipe.candidate, queuedConfig: config } };
      return { ...s, recipe: { ...s.recipe, production: s.recipe.candidate }, productionConfig: config };
    }
    case "live-next":
      if (s.live.phase !== 4 || !s.live.accepting || !s.live.continuous || !s.live.auto) return s;
      return reduce(s, { type: "live-start" });
    case "live-start": {
      if (s.recipe.production < 1 || !s.device.connected || !s.device.applied || !s.plc.ready || !s.plc.connected || (s.live.phase > 0 && s.live.phase < 4)) return s;
      return { ...s, selectedFrame: 1, live: { ...s.live, accepting: true, phase: 1, auto: true, result: null, inFlightVersion: s.recipe.production, inFlightConfig: s.productionConfig, part: s.live.part + 1 } };
    }
    case "live-stop": return { ...s, live: { ...s.live, accepting: false } };
    case "live-step": {
      if (s.live.phase < 1 || s.live.phase >= 4) return s;
      const finish = s.live.phase === 3;
      return { ...s, productionConfig: finish && s.live.queuedConfig ? s.live.queuedConfig : s.productionConfig, recipe: finish && s.live.queued !== null ? { ...s.recipe, production: s.live.queued } : s.recipe, live: { ...s.live, accepting: finish && !s.live.continuous ? false : s.live.accepting, phase: s.live.phase + 1, result: finish ? s.live.scenario : null, queued: finish ? null : s.live.queued, queuedConfig: finish ? null : s.live.queuedConfig } };
    }
    case "live-option": return { ...s, live: { ...s.live, ...a.patch, scenario: working(s) ? s.live.scenario : a.patch.scenario ?? s.live.scenario } };
    case "record": return { ...s, record: a.id };
    case "compare": {
      const record = historyRecords.find(r => r.id === s.record);
      if (!record || (a.kind === "remeasure" && !record.raw)) return s;
      const gap = a.kind === "remeasure" ? 0 : record.gap;
      const verdict: Verdict = record.result === "ERR" ? "ERR" : gap > s.recipe.maxGap ? "NG" : "OK";
      return { ...s, comparisons: { ...s.comparisons, [record.id]: { kind: a.kind, verdict, gap, version: s.recipe.candidate } } };
    }
    case "settings": return { ...s, settings: { ...s.settings, ...a.patch } };
  }
}

export function sceneState(id: string): WorkflowState {
  const s = initialState(); s.scene = id;
  const f = s.frames[2];
  if (id.startsWith("teach-")) {
    if (id === "teach-empty" || id === "teach-stale") f.imageId = null;
    if (id === "teach-pass" || id === "teach-saved") f.trial = { imageId: f.imageId!, revision: f.revision, pass: true, score: 0.94 };
    if (id === "teach-saved") f.saved = true;
    if (id === "teach-fail") { f.quality = "low"; f.trial = { imageId: f.imageId!, revision: f.revision, pass: false, score: 0.31 }; }
    if (id === "teach-k4") s.selectedFrame = 4;
    if (id === "teach-history") { f.backup = { imageId: f.imageId, source: f.source, sourceRecord: f.sourceRecord, quality: f.quality, captureSettings: { ...f.captureSettings }, params: { ...f.params } }; f.imageId = 184003; f.source = "history"; f.sourceRecord = "TJ-000184"; }
  }
  if (id === "coverage-fail") s.recipe.fovWidth = 170;
  if (id === "calibration-fail") s.calibration = { captured: true, result: "fail", saved: false, sample: "bad", version: 7 };
  if (id.startsWith("validation-") || id === "publish-confirm") {
    if (id !== "validation-blocked") { f.trial = { imageId: f.imageId!, revision: f.revision, pass: true, score: 0.94 }; f.saved = true; }
    if (id === "validation-pass" || id === "validation-released" || id === "publish-confirm") s.validation = { status: "passed", revision: s.recipe.revision };
    if (id === "validation-released") { s.recipe.production = 14; s.productionConfig = runtimeConfig(s.recipe, s.frames, s.overview, "C07", 14); }
  }
  if (id.startsWith("live-")) {
    s.live.inFlightConfig = s.productionConfig;
    if (id === "live-running") { s.live.phase = 2; s.live.accepting = true; s.live.inFlightVersion = 13; }
    if (id === "live-ng" || id === "live-k4" || id === "live-err") {
      s.live.phase = 4; s.live.inFlightVersion = 13; s.live.result = id === "live-err" ? "ERR" : "NG"; s.live.scenario = s.live.result;
    }
    if (id === "live-k4") s.selectedFrame = 4;
  }
  if (id === "history-no-raw") s.record = "TJ-000182";
  if (id === "history-compare") s.comparisons["TJ-000184"] = { kind: "remeasure", verdict: "OK", gap: 0, version: 14 };
  return { ...s, recipeLibrary: s.recipeLibrary.map(entry => entry.recipe.name === s.recipe.name ? workspaceOf(s) : entry) };
}

// Storage is optional; reject broken nested data before any view reads it.
type StoredObject = Record<string, unknown>;
const object = (value: unknown): value is StoredObject => value !== null && typeof value === "object" && !Array.isArray(value);
const numbers = (value: unknown, keys: string[]) => object(value) && keys.every(key => typeof value[key] === "number" && Number.isFinite(value[key]));
const booleans = (value: unknown, keys: string[]) => object(value) && keys.every(key => typeof value[key] === "boolean");
const verdict = (value: unknown) => value === "OK" || value === "NG" || value === "ERR";
const nullableNumber = (value: unknown) => value === null || typeof value === "number" && Number.isFinite(value);
const validParams = (value: unknown) => numbers(value, ["search", "contrast", "minWidth", "maxWidth"]);
const validRecipe = (value: unknown) => object(value) && typeof value.name === "string" && Boolean(value.name.trim()) && numbers(value, ["candidate", "production", "revision", "width", "height", "radius", "fovWidth", "fovHeight", "speed", "target", "tolerance", "maxGap"]);
function validOverview(value: unknown): boolean {
  return object(value) && typeof value.saved === "boolean" && (value.background === null || typeof value.background === "string") && Array.isArray(value.positions) && value.positions.length === 6 && value.positions.every(p => numbers(p, ["x", "y"]));
}
function validRuntime(value: unknown): boolean {
  return object(value) && numbers(value, ["version"]) && validRecipe(value.recipe) && Array.isArray(value.frameParams) && value.frameParams.length === 6 && value.frameParams.every(validParams) && typeof value.calibrationVersion === "string" && validOverview(value.overview);
}
function validFrame(value: unknown, index: number): boolean {
  if (!object(value) || value.id !== index + 1 || !nullableNumber(value.imageId) || !numbers(value, ["revision"]) || !validParams(value.params) || typeof value.saved !== "boolean" || !["camera", "history"].includes(String(value.source)) || !["normal", "low"].includes(String(value.quality)) || value.sourceRecord !== null && typeof value.sourceRecord !== "string") return false;
  if (!object(value.captureSettings) || !numbers(value.captureSettings, ["exposure"]) || typeof value.captureSettings.calibrationVersion !== "string") return false;
  if (value.trial !== null && (!numbers(value.trial, ["imageId", "revision", "score"]) || !booleans(value.trial, ["pass"]))) return false;
  if (value.backup !== null) {
    const backup = value.backup;
    if (!object(backup) || !nullableNumber(backup.imageId) || !validParams(backup.params) || !["camera", "history"].includes(String(backup.source)) || !["normal", "low"].includes(String(backup.quality)) || backup.sourceRecord !== null && typeof backup.sourceRecord !== "string" || !object(backup.captureSettings) || !numbers(backup.captureSettings, ["exposure"]) || typeof backup.captureSettings.calibrationVersion !== "string") return false;
  }
  return true;
}
function validWorkspace(value: unknown): boolean {
  return object(value) && validRecipe(value.recipe) && validRuntime(value.productionConfig) && Array.isArray(value.frames) && value.frames.length === 6 && value.frames.every(validFrame) && validOverview(value.overview) && object(value.validation) && ["idle", "passed", "failed"].includes(String(value.validation.status)) && nullableNumber(value.validation.revision);
}
export function restorePreview(value: unknown): WorkflowState {
  const fallback = initialState();
  if (!object(value) || value.schema !== 1 || !validWorkspace(value) || typeof value.scene !== "string" || !Number.isInteger(value.selectedFrame) || Number(value.selectedFrame) < 1 || Number(value.selectedFrame) > 6 || !numbers(value, ["imageSequence"])) return fallback;
  if (!booleans(value.device, ["connected", "applied"]) || !numbers(value.device, ["exposure", "gain"]) || !object(value.device) || typeof value.device.trigger !== "string") return fallback;
  if (!object(value.plc) || !booleans(value.plc, ["connected", "ready"]) || typeof value.plc.address !== "string" || !["S7", "Modbus TCP"].includes(String(value.plc.protocol))) return fallback;
  if (value.plc.points !== undefined && (!Array.isArray(value.plc.points) || value.plc.points.length !== 6 || !value.plc.points.every(p => typeof p === "string"))) return fallback;
  if (value.plc.pointsApplied !== undefined && typeof value.plc.pointsApplied !== "boolean") return fallback;
  if (!object(value.calibration) || !booleans(value.calibration, ["captured", "saved"]) || !numbers(value.calibration, ["version"]) || !["idle", "pass", "fail"].includes(String(value.calibration.result)) || !["good", "bad"].includes(String(value.calibration.sample))) return fallback;
  if (!object(value.live) || !booleans(value.live, ["accepting", "auto"]) || !Number.isInteger(value.live.phase) || Number(value.live.phase) < 0 || Number(value.live.phase) > 4 || !numbers(value.live, ["part"]) || !verdict(value.live.scenario) || value.live.result !== null && !verdict(value.live.result) || !nullableNumber(value.live.inFlightVersion) || !nullableNumber(value.live.queued) || value.live.inFlightConfig !== null && !validRuntime(value.live.inFlightConfig) || value.live.queuedConfig !== null && !validRuntime(value.live.queuedConfig)) return fallback;
  if (value.live.continuous !== undefined && typeof value.live.continuous !== "boolean") return fallback;
  if (!numbers(value.settings, ["retention", "timeout"]) || !booleans(value.settings, ["saved"]) || !object(value.settings) || typeof value.settings.raw !== "string" || typeof value.record !== "string" || !object(value.comparisons) || !Object.values(value.comparisons).every(c => object(c) && ["remeasure", "rejudge"].includes(String(c.kind)) && verdict(c.verdict) && numbers(c, ["gap", "version"]))) return fallback;
  if (value.recipeLibrary !== undefined && (!Array.isArray(value.recipeLibrary) || !value.recipeLibrary.length || !value.recipeLibrary.every(validWorkspace))) return fallback;
  const state = value as unknown as WorkflowState;
  const library = state.recipeLibrary ?? fallback.recipeLibrary.filter(entry => entry.recipe.name !== state.recipe.name);
  const names = library.map(entry => entry.recipe.name);
  if (new Set(names).size !== names.length) return fallback;
  const workspace = workspaceOf(state);
  return { ...state, plc: { ...state.plc, points: state.plc.points ?? plcPoints(state.plc.protocol), pointsApplied: state.plc.pointsApplied ?? true }, live: { ...state.live, continuous: state.live.continuous ?? false }, recipeLibrary: names.includes(state.recipe.name) ? library.map(entry => entry.recipe.name === state.recipe.name ? workspace : entry) : [...library, workspace] };
}
