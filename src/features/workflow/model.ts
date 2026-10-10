export type View = "guide" | "device" | "plc" | "calibration" | "recipes" | "geometry" | "teach" | "overview" | "validation" | "live" | "history" | "record" | "settings";
export type Verdict = "OK" | "NG" | "ERR";
export type Point = [number, number];
export type Trial = { imageId: number; revision: number; pass: boolean; score: number };
export interface RecipeConfiguration {
  id: string; name: string; productCode: number; candidate: number; production: number; revision: number;
  spacing: number; target: number; tolerance: number; maxGap: number;
}
export interface PreviewCamera {
  id: string; connected: boolean; applied: boolean; exposure: number; gain: number; trigger: string;
  viewCount: 1 | 3; size: [number, number];
}
export interface Calibration {
  captured: boolean; result: "idle" | "pass" | "fail"; saved: boolean; sample: "good" | "bad"; version: number;
}
export interface TeachFrame {
  id: number; shotId: string; poseId: string; camera: string; view: number; skip: boolean;
  path: Point[]; mmPerPx: number; size: [number, number];
  imageId: number | null;
  source: "camera" | "history"; sourceRecord: string | null; quality: "normal" | "low";
  captureSettings: { exposure: number; calibrationVersion: string };
  revision: number; trial: Trial | null; saved: boolean;
  backup: Pick<TeachFrame, "imageId" | "source" | "sourceRecord" | "quality" | "captureSettings" | "params" | "path" | "mmPerPx"> | null;
  params: { search: number; polarity: "dark" | "light"; minWidth: number; maxWidth: number; gapLimit: number | null };
}
export type FrozenShot = Pick<TeachFrame, "id" | "shotId" | "poseId" | "camera" | "view" | "skip" | "path" | "mmPerPx" | "size" | "params" | "imageId" | "captureSettings">;
export interface RuntimeConfiguration {
  version: number; recipe: RecipeConfiguration; shots: FrozenShot[];
  bundleHash: string; recipeHash: string; graphVersion: string; engineVersion: string; calibrationVersion: string;
  overview: { positions: { x: number; y: number }[]; background: string | null; saved: boolean };
}
export interface Comparison {
  kind: "remeasure" | "rejudge"; mode: "original" | "candidate"; verdict: Verdict; gap: number; version: number;
  cycleId: string; bundleHash: string;
}
export interface WorkflowState {
  schema: 4; scene: string; device: PreviewCamera; cameras: PreviewCamera[];
  plc: { connected: boolean; ready: boolean; address: string; protocol: string; points: string[]; pointsApplied: boolean };
  calibration: Calibration; calibrations: Record<string, Calibration>;
  recipe: RecipeConfiguration; recipeLibrary: RecipeWorkspace[]; productionConfig: RuntimeConfiguration;
  frames: TeachFrame[]; selectedFrame: number; imageSequence: number; overview: RuntimeConfiguration["overview"];
  validation: { status: "idle" | "passed" | "failed"; revision: number | null };
  live: { accepting: boolean; phase: number; auto: boolean; continuous: boolean; scenario: Verdict; result: Verdict | null; inFlightVersion: number | null; inFlightConfig: RuntimeConfiguration | null; queued: number | null; queuedConfig: RuntimeConfiguration | null; part: number; cycleId: string | null };
  record: string; comparisons: Record<string, Comparison>;
  settings: { retention: number; timeout: number; raw: string; saved: boolean };
}
export type RecipeWorkspace = Pick<WorkflowState, "recipe" | "productionConfig" | "frames" | "overview" | "validation">;
export const plcPoints = (protocol: string) => protocol === "Modbus TCP" ? ["00001", "00002", "00003", "00004", "40001", "00005"] : ["DB20.DBX0.0", "DB20.DBX0.1", "DB20.DBX0.2", "DB20.DBX0.3", "DB20.DBW2", "DB20.DBX4.0"];
export const initialPositions = [{ x: 0.18, y: 0.21 }, { x: 0.5, y: 0.21 }, { x: 0.82, y: 0.21 }, { x: 0.82, y: 0.79 }, { x: 0.5, y: 0.79 }, { x: 0.18, y: 0.79 }];
export const defaultRecipe: RecipeConfiguration = { id: "DEMO-A", name: "工件 A · 壳体", productCode: 12, candidate: 14, production: 13, revision: 1, spacing: 1, target: 0, tolerance: 1, maxGap: 0.5 };
export const defaultParams: TeachFrame["params"] = { search: 4, polarity: "light", minWidth: 2.5, maxWidth: 5, gapLimit: null };
export function examplePath(id: number, size: [number, number] = id === 4 ? [1600, 1200] : [1280, 1024]): Point[] {
  const paths = [
    [[86, 366], [86, 152], [90, 122], [103, 101], [125, 88], [155, 84], [684, 84]],
    [[34, 204], [686, 204]],
    [[36, 84], [552, 84], [585, 89], [609, 103], [623, 127], [628, 160], [628, 379]],
    [[626, 36], [626, 284], [622, 314], [609, 335], [587, 348], [558, 352], [34, 352]],
    [[34, 238], [686, 238]],
    [[86, 36], [86, 286], [90, 316], [103, 337], [125, 350], [155, 352], [684, 352]],
  ];
  return paths[(id - 1) % 6].map(([x, y]) => [Math.round(x / 720 * size[0]), Math.round(y / 430 * size[1])]);
}
function exampleFrames(): TeachFrame[] {
  return Array.from({ length: 6 }, (_, i) => ({
    id: i + 1, shotId: "P" + (i + 1), poseId: "Pose-" + (i + 1), camera: i === 3 ? "cam2" : i === 4 ? "cam3" : "cam1", view: i < 3 ? i + 1 : 1, skip: false,
    path: examplePath(i + 1), mmPerPx: 0.112, size: i === 3 ? [1600, 1200] : [1280, 1024],
    imageId: 41 + i, source: "camera", sourceRecord: null, quality: "normal", captureSettings: { exposure: 60, calibrationVersion: "C07" }, revision: 1,
    trial: i === 2 ? null : { imageId: 41 + i, revision: 1, pass: true, score: 0.94 }, saved: i !== 2, backup: null, params: { ...defaultParams },
  }));
}
function fingerprint(value: unknown): string {
  let hash = 2166136261;
  for (const char of JSON.stringify(value)) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619);
  return (hash >>> 0).toString(16).padStart(8, "0");
}
export function runtimeConfig(recipe: RecipeConfiguration, frames: TeachFrame[], overview: WorkflowState["overview"], version: number): RuntimeConfiguration {
  const shots = frames.map(({ id, shotId, poseId, camera, view, skip, path, mmPerPx, size, params, imageId, captureSettings }) => structuredClone({ id, shotId, poseId, camera, view, skip, path, mmPerPx, size, params, imageId, captureSettings }));
  const recipeHash = "demo-recipe-" + fingerprint([{ id: recipe.id, name: recipe.name, productCode: recipe.productCode, spacing: recipe.spacing, target: recipe.target, tolerance: recipe.tolerance, maxGap: recipe.maxGap }, shots.map(f => [f.shotId, f.poseId, f.camera, f.view, f.skip, f.path, f.mmPerPx, f.size, f.params])]);
  const graphVersion = "taught-path-1", engineVersion = "示例引擎 · 无 DLL";
  return { version, recipe: { ...recipe }, shots, recipeHash, bundleHash: "demo-bundle-" + fingerprint([version, recipeHash, graphVersion, engineVersion, shots, overview]), graphVersion, engineVersion, calibrationVersion: [...new Set(shots.filter(f => !f.skip).map(f => f.camera + ":" + f.captureSettings.calibrationVersion))].join(" / "), overview: structuredClone(overview) };
}
export function initialState(): WorkflowState {
  const recipe = { ...defaultRecipe }, frames = exampleFrames();
  const overview = { positions: initialPositions.map(p => ({ ...p })), background: null, saved: false };
  const workspace: RecipeWorkspace = { recipe, frames, overview, productionConfig: runtimeConfig(recipe, frames, overview, 13), validation: { status: "idle", revision: null } };
  const other = (name: string, production: number): RecipeWorkspace => {
    const r = { ...recipe, id: "DEMO-B", productCode: 13, name, candidate: production + 1, production };
    const otherFrames = structuredClone(frames), otherOverview = structuredClone(overview);
    return { ...workspace, frames: otherFrames, overview: otherOverview, recipe: r, productionConfig: runtimeConfig(r, otherFrames, otherOverview, production) };
  };
  const cameras: PreviewCamera[] = [1, 2, 3].map(n => ({ id: "cam" + n, connected: true, applied: true, exposure: 60, gain: 6, trigger: "Line0 · 上升沿", viewCount: n === 1 ? 3 : 1, size: n === 2 ? [1600, 1200] : [1280, 1024] }));
  const calibration: Calibration = { captured: true, result: "pass", saved: true, sample: "good", version: 7 };
  return {
    schema: 4, scene: "default", device: cameras[0], cameras,
    plc: { connected: true, ready: true, address: "192.168.1.10", protocol: "S7", points: plcPoints("S7"), pointsApplied: true },
    calibration, calibrations: Object.fromEntries(cameras.map(c => [c.id, { ...calibration }])),
    ...workspace, recipeLibrary: [workspace, other("工件 B · 底板", 8)], selectedFrame: 3, imageSequence: 50,
    live: { accepting: false, phase: 0, auto: false, continuous: false, scenario: "NG", result: null, inFlightVersion: null, inFlightConfig: null, queued: null, queuedConfig: null, part: 184, cycleId: null },
    record: "TJ-000184", comparisons: {}, settings: { retention: 30, timeout: 3000, raw: "NG 与 ERR", saved: true },
  };
}
export const cameraFor = (s: WorkflowState, id: string) => id === s.device.id ? s.device : s.cameras.find(c => c.id === id);
const calibrationFor = (s: WorkflowState, id: string) => id === s.device.id ? s.calibration : s.calibrations[id];
export function shotProblem(s: WorkflowState, f: TeachFrame): string | null {
  const camera = cameraFor(s, f.camera), p = f.params;
  if (!f.shotId.trim() || !f.poseId.trim() || s.frames.some(other => other.id !== f.id && other.shotId === f.shotId)) return "拍照点 ID / Pose 不能为空，ID 不得重复";
  if (!camera || !Number.isInteger(f.view) || f.view < 1 || f.view > camera.viewCount) return "所选设备没有该视角";
  if (f.skip) return null;
  if (!Number.isFinite(f.mmPerPx) || f.mmPerPx <= 0 || f.mmPerPx > 10) return "请确认本点 mmPerPx";
  if (f.path.length < 2 || f.path.some(p => p.some(n => !Number.isFinite(n)) || p[0] < 0 || p[1] < 0 || p[0] >= f.size[0] || p[1] >= f.size[1])) return "像素中线需要至少两个点，且必须位于原图内";
  const length = f.path.slice(1).reduce((sum, p, i) => sum + Math.hypot(p[0] - f.path[i][0], p[1] - f.path[i][1]), 0);
  if (length < 8 || s.recipe.spacing / f.mmPerPx < 1 || Math.floor(length * f.mmPerPx / s.recipe.spacing) < 2) return "中线至少 8 px，站距至少 1 px，且需三个测点";
  if (![p.search, p.minWidth, p.maxWidth].every(Number.isFinite) || p.minWidth <= 0 || p.minWidth >= p.maxWidth || p.maxWidth >= 2 * p.search) return "需满足 0 < 胶宽下限 < 上限 < 两倍搜索半宽";
  if (p.gapLimit !== null && (!Number.isFinite(p.gapLimit) || p.gapLimit < 0)) return "本点允许断胶长度必须大于等于零";
  return null;
}
export const coverage = (s: WorkflowState) => Math.round(s.frames.filter(f => !shotProblem(s, f)).length / s.frames.length * 1000) / 10;
export function canSaveFrame(f: TeachFrame): boolean {
  return f.skip || f.imageId !== null && f.trial?.pass === true && f.trial.imageId === f.imageId && f.trial.revision === f.revision;
}
export function validationChecks(s: WorkflowState): { label: string; pass: boolean; view: View; detail: string }[] {
  const cameras = [...new Set(s.frames.map(f => f.camera))];
  return [
    { label: "参与设备与握手就绪", pass: cameras.every(id => { const c = cameraFor(s, id); return c?.connected && c.applied; }) && s.plc.connected && s.plc.ready, view: "device", detail: cameras.join(" / ") + " · 按设备计触发，不按视角加拍" },
    { label: "参与工位标定有效", pass: cameras.every(id => calibrationFor(s, id)?.saved), view: "calibration", detail: cameras.map(id => id + ":C" + String(calibrationFor(s, id)?.version ?? 0).padStart(2, "0")).join(" / ") },
    { label: "逐点规划与像素中线有效", pass: coverage(s) === 100, view: "geometry", detail: s.frames.filter(f => !shotProblem(s, f)).length + " / " + s.frames.length + " 拍照点 · Pose / camera / view / 像素比例" },
    { label: "所有拍照点示教已保存", pass: s.frames.every(f => f.saved && canSaveFrame(f)), view: "teach", detail: s.frames.filter(f => f.saved && canSaveFrame(f)).length + " / " + s.frames.length + " 拍照点" },
  ];
}
export const canPublish = (s: WorkflowState) => s.validation.status === "passed" && s.validation.revision === s.recipe.revision && validationChecks(s).every(c => c.pass) && s.recipe.candidate !== s.recipe.production && s.live.queued === null;
export const validationSamples = [
  { id: "S01", name: "正常胶路", expected: "OK" as Verdict, d: 0.3, width: 3.94, gap: 0, valid: true },
  { id: "S02", name: "断胶样本", expected: "NG" as Verdict, d: 0.2, width: 3.8, gap: 6.2, valid: true },
  { id: "S03", name: "中线偏离", expected: "NG" as Verdict, d: 2.2, width: 3.9, gap: 0, valid: true },
  { id: "S04", name: "胶宽超限", expected: "NG" as Verdict, d: 0.2, width: 5.8, gap: 0, valid: true },
  { id: "S05", name: "测量失败", expected: "ERR" as Verdict, d: 0, width: 0, gap: 0, valid: false },
];
export function sampleVerdict(s: WorkflowState, sample: typeof validationSamples[number]): Verdict {
  if (!sample.valid) return "ERR";
  const p = s.frames[2].params;
  return Math.abs(sample.d - s.recipe.target) > s.recipe.tolerance || sample.width < p.minWidth || sample.width > p.maxWidth || sample.gap > (p.gapLimit ?? s.recipe.maxGap) ? "NG" : "OK";
}
const originalConfig = runtimeConfig(defaultRecipe, exampleFrames(), { positions: initialPositions, background: null, saved: true }, 13);
export const historyRecords = [
  { id: "TJ-000184", cycleId: "demo-cycle-184", time: "2026-10-08 14:32:08", result: "NG" as Verdict, cause: "P3 断胶 · 6.2 mm", raw: true, frames: 6, version: 13, gap: 6.2, bundleHash: originalConfig.bundleHash, config: originalConfig },
  { id: "TJ-000183", cycleId: "demo-cycle-183", time: "2026-10-08 14:31:52", result: "OK" as Verdict, cause: "胶路合格", raw: true, frames: 6, version: 13, gap: 0, bundleHash: originalConfig.bundleHash, config: originalConfig },
  { id: "TJ-000182", cycleId: "demo-cycle-182", time: "2026-10-07 09:22:14", result: "OK" as Verdict, cause: "仅测量数据", raw: false, frames: 6, version: 13, gap: 0, bundleHash: originalConfig.bundleHash, config: originalConfig },
  { id: "TJ-000181", cycleId: "demo-cycle-181", time: "2026-10-07 09:21:57", result: "ERR" as Verdict, cause: "P3 原图缺失", raw: true, frames: 5, version: 13, gap: 0, bundleHash: originalConfig.bundleHash, config: originalConfig },
];
export function layoutCompatible(s: WorkflowState, config: RuntimeConfiguration): boolean {
  const layout = (frames: (TeachFrame | FrozenShot)[]) => frames.map(f => [f.shotId, f.poseId, f.camera, f.view, f.skip, f.path, f.mmPerPx]);
  return s.recipe.spacing === config.recipe.spacing && JSON.stringify(layout(s.frames)) === JSON.stringify(layout(config.shots));
}
export type Action =
  | { type: "load"; state: WorkflowState } | { type: "device"; patch: Partial<PreviewCamera> } | { type: "device-select"; id: string }
  | { type: "plc"; patch: Partial<WorkflowState["plc"]> } | { type: "calibration"; patch: Partial<Calibration> }
  | { type: "recipe"; patch: Partial<RecipeConfiguration> } | { type: "recipe-create"; name: string; copy: boolean } | { type: "recipe-open"; name: string } | { type: "recipe-delete"; name: string }
  | { type: "select-frame"; id: number } | { type: "capture"; quality?: "normal" | "low"; history?: boolean }
  | { type: "frame-plan"; patch: Partial<Pick<TeachFrame, "shotId" | "poseId" | "camera" | "view" | "skip">> }
  | { type: "frame-param"; key: "search" | "minWidth" | "maxWidth"; value: number }
  | { type: "frame-gap"; value: number | null }
  | { type: "frame-polarity"; value: "dark" | "light" } | { type: "frame-path"; path: Point[] } | { type: "frame-scale"; value: number }
  | { type: "trial" } | { type: "save-frame" } | { type: "restore-frame" } | { type: "overview"; patch: Partial<WorkflowState["overview"]> }
  | { type: "validate" } | { type: "publish" } | { type: "live-start" } | { type: "live-next" } | { type: "live-stop" } | { type: "live-step" }
  | { type: "live-option"; patch: Partial<WorkflowState["live"]> } | { type: "record"; id: string }
  | { type: "compare"; kind: Comparison["kind"]; mode?: Comparison["mode"] } | { type: "settings"; patch: Partial<WorkflowState["settings"]> };
function changed(s: WorkflowState): WorkflowState {
  return { ...s, recipe: { ...s.recipe, revision: s.recipe.revision + 1, candidate: Math.max(s.recipe.candidate, s.recipe.production + 1, (s.live.queued ?? 0) + 1) }, validation: { status: "idle", revision: null } };
}
const working = (s: WorkflowState) => s.live.phase > 0 && s.live.phase < 4;
function workspaceOf(s: WorkflowState): RecipeWorkspace {
  return { recipe: s.recipe, productionConfig: s.productionConfig, frames: s.frames, overview: s.overview, validation: s.validation };
}
const invalidate = (f: TeachFrame, recapture = false): TeachFrame => ({ ...f, imageId: recapture ? null : f.imageId, trial: null, saved: false, revision: f.revision + 1, backup: recapture ? null : f.backup });
function editFrame(s: WorkflowState, edit: (f: TeachFrame) => TeachFrame): WorkflowState {
  if (working(s) && s.recipe.candidate === s.recipe.production) return s;
  return changed({ ...s, frames: s.frames.map(f => f.id === s.selectedFrame ? edit(f) : f) });
}
export function reducer(s: WorkflowState, a: Action): WorkflowState {
  const next = reduce(s, a);
  if (next === s) return s;
  const workspace = workspaceOf(next);
  const recapture = a.type === "device" && ["exposure", "gain", "trigger", "viewCount"].some(k => k in a.patch) || a.type === "calibration" && a.patch.saved;
  return { ...next, recipeLibrary: next.recipeLibrary.map(entry => {
    if (entry.recipe.name === next.recipe.name) return workspace;
    if (!recapture || !entry.frames.some(f => f.camera === s.device.id)) return entry;
    return { ...entry, recipe: { ...entry.recipe, revision: entry.recipe.revision + 1, candidate: Math.max(entry.recipe.candidate, entry.recipe.production + 1) }, validation: { status: "idle", revision: null }, frames: entry.frames.map(f => f.camera === s.device.id ? invalidate(f, true) : f) };
  }) };
}
function reduce(s: WorkflowState, a: Action): WorkflowState {
  switch (a.type) {
    case "load": return a.state;
    case "device-select": {
      if (working(s)) return s;
      const device = cameraFor(s, a.id);
      if (!device) return s;
      return { ...s, device, cameras: s.cameras.map(c => c.id === s.device.id ? s.device : c), calibration: calibrationFor(s, a.id)!, calibrations: { ...s.calibrations, [s.device.id]: s.calibration } };
    }
    case "device": {
      if (working(s)) return s;
      const edit = ["exposure", "gain", "trigger", "viewCount"].some(k => k in a.patch);
      const device = { ...s.device, ...a.patch, applied: edit ? false : a.patch.applied ?? s.device.applied };
      const next = { ...s, device, cameras: s.cameras.map(c => c.id === device.id ? device : c) };
      return edit && s.frames.some(f => f.camera === device.id) ? changed({ ...next, frames: s.frames.map(f => f.camera === device.id ? invalidate(f, true) : f) }) : next;
    }
    case "plc": return working(s) ? s : { ...s, plc: { ...s.plc, ...a.patch } };
    case "calibration": {
      if (working(s) || a.patch.saved && (s.calibration.saved || s.calibration.result !== "pass")) return s;
      const calibration = { ...s.calibration, ...a.patch, version: a.patch.saved ? s.calibration.version + 1 : s.calibration.version };
      const next = { ...s, calibration, calibrations: { ...s.calibrations, [s.device.id]: calibration } };
      return a.patch.saved && s.frames.some(f => f.camera === s.device.id) ? changed({ ...next, frames: s.frames.map(f => f.camera === s.device.id ? invalidate(f, true) : f) }) : next;
    }
    case "recipe": {
      if (working(s) && s.recipe.candidate === s.recipe.production && a.patch.candidate === undefined) return s;
      const next = changed({ ...s, recipe: { ...s.recipe, ...a.patch } });
      return a.patch.spacing === undefined ? next : { ...next, frames: next.frames.map(f => f.skip ? f : invalidate(f)) };
    }
    case "recipe-create": {
      const name = a.name.trim();
      if (!name || working(s) || s.live.queued !== null || s.recipeLibrary.some(entry => entry.recipe.name === name)) return s;
      const recipe = { ...(a.copy ? s.recipe : defaultRecipe), id: "DEMO-" + crypto.randomUUID(), productCode: Math.max(19, ...s.recipeLibrary.map(entry => entry.recipe.productCode)) + 1, name, candidate: 1, production: 0, revision: 1 };
      const frames = (a.copy ? s.frames : exampleFrames()).map(f => ({ ...structuredClone(f), imageId: null, source: "camera" as const, sourceRecord: null, quality: "normal" as const, revision: 1, trial: null, saved: false, backup: null }));
      const overview = { positions: initialPositions.map(p => ({ ...p })), background: null, saved: false };
      const workspace: RecipeWorkspace = { recipe, frames, overview, validation: { status: "idle", revision: null }, productionConfig: runtimeConfig(recipe, frames, overview, 0) };
      return { ...s, ...workspace, selectedFrame: 1, recipeLibrary: [...s.recipeLibrary, workspace] };
    }
    case "recipe-open": {
      const entry = s.recipeLibrary.find(entry => entry.recipe.name === a.name);
      if (!entry || working(s) && a.name !== s.recipe.name || s.live.queued !== null) return s;
      const next = { ...s, ...entry, selectedFrame: 1 };
      return next.recipe.candidate === next.recipe.production ? changed(next) : next;
    }
    case "recipe-delete": {
      if (working(s) || s.live.queued !== null || s.recipeLibrary.length <= 1) return s;
      const recipeLibrary = s.recipeLibrary.filter(entry => entry.recipe.name !== a.name);
      if (recipeLibrary.length === s.recipeLibrary.length) return s;
      return { ...s, ...(a.name === s.recipe.name ? recipeLibrary[0] : {}), recipeLibrary, selectedFrame: a.name === s.recipe.name ? 1 : s.selectedFrame };
    }
    case "select-frame": return Number.isInteger(a.id) && s.frames.some(f => f.id === a.id) ? { ...s, selectedFrame: a.id } : s;
    case "frame-plan": return editFrame(s, f => {
      const camera = cameraFor(s, a.patch.camera ?? f.camera);
      return { ...invalidate(f, true), ...a.patch, view: a.patch.camera && a.patch.camera !== f.camera ? 1 : a.patch.view ?? f.view, size: camera?.size ?? f.size };
    });
    case "capture": {
      if (working(s) && s.recipe.candidate === s.recipe.production) return s;
      const f = s.frames[s.selectedFrame - 1], record = a.history ? historyRecords.find(r => r.id === s.record) : null;
      const source = record?.config.shots[f.id - 1], camera = cameraFor(s, f.camera);
      if (f.skip || (a.history ? !record?.raw || record.result === "ERR" && f.id === 3 || source?.camera !== f.camera || source.view !== f.view || source.shotId !== f.shotId || source.poseId !== f.poseId : !camera?.connected || !camera.applied || f.view < 1 || f.view > camera.viewCount)) return s;
      return editFrame({ ...s, imageSequence: s.imageSequence + 1 }, current => ({
        ...invalidate(current), imageId: record ? Number(record.id.replace("TJ-", "")) * 1000 + f.id : s.imageSequence + 1,
        source: record ? "history" : "camera", sourceRecord: record?.id ?? null, quality: a.quality ?? "normal",
        captureSettings: { exposure: record ? 60 : camera!.exposure, calibrationVersion: record ? "C07" : "C" + String(calibrationFor(s, f.camera)?.version ?? 0).padStart(2, "0") },
        backup: record ? current.backup ?? structuredClone({ imageId: current.imageId, source: current.source, sourceRecord: current.sourceRecord, quality: current.quality, captureSettings: current.captureSettings, params: current.params, path: current.path, mmPerPx: current.mmPerPx }) : current.backup,
      }));
    }
    case "frame-param": return editFrame(s, f => ({ ...invalidate(f), params: { ...f.params, [a.key]: a.value } }));
    case "frame-gap": return working(s) && s.recipe.candidate === s.recipe.production ? s : changed({ ...s, frames: s.frames.map(f => f.id === s.selectedFrame ? { ...f, params: { ...f.params, gapLimit: a.value } } : f) });
    case "frame-polarity": return editFrame(s, f => ({ ...invalidate(f), params: { ...f.params, polarity: a.value } }));
    case "frame-path": return editFrame(s, f => ({ ...invalidate(f), path: structuredClone(a.path) }));
    case "frame-scale": return editFrame(s, f => ({ ...invalidate(f), mmPerPx: a.value }));
    case "trial": return working(s) && s.recipe.candidate === s.recipe.production ? s : { ...s, frames: s.frames.map(f => f.id !== s.selectedFrame || f.imageId === null || f.skip ? f : { ...f, saved: false, trial: { imageId: f.imageId, revision: f.revision, pass: !shotProblem(s, f) && f.quality === "normal", score: f.quality === "normal" && !shotProblem(s, f) ? 0.94 : 0.31 } }) };
    case "save-frame": {
      if (working(s) && s.recipe.candidate === s.recipe.production) return s;
      const f = s.frames[s.selectedFrame - 1];
      return canSaveFrame(f) && !f.saved && !shotProblem(s, f) ? changed({ ...s, frames: s.frames.map(current => current.id === f.id ? { ...current, saved: true } : current) }) : s;
    }
    case "restore-frame": return editFrame(s, f => ({ ...invalidate(f), ...(f.backup ?? { params: { ...defaultParams }, path: examplePath(f.id, f.size), mmPerPx: 0.112 }), backup: null }));
    case "overview": return { ...s, overview: { ...s.overview, ...a.patch } };
    case "validate": return validationChecks(s).every(c => c.pass) ? { ...s, validation: { status: validationSamples.every(sample => sampleVerdict(s, sample) === sample.expected) ? "passed" : "failed", revision: s.recipe.revision } } : s;
    case "publish": {
      if (!canPublish(s)) return s;
      const config = runtimeConfig(s.recipe, s.frames, s.overview, s.recipe.candidate);
      return working(s) ? { ...s, live: { ...s.live, queued: config.version, queuedConfig: config } } : { ...s, recipe: { ...s.recipe, production: config.version }, productionConfig: config };
    }
    case "live-next": return s.live.phase === 4 && s.live.accepting && s.live.continuous && s.live.auto ? reduce(s, { type: "live-start" }) : s;
    case "live-start": {
      if (s.recipe.production < 1 || !s.productionConfig.shots.every(f => { const c = cameraFor(s, f.camera); return c?.connected && c.applied && f.view <= c.viewCount; }) || !s.plc.ready || !s.plc.connected || working(s)) return s;
      return { ...s, selectedFrame: 1, live: { ...s.live, accepting: true, phase: 1, auto: true, result: null, inFlightVersion: s.recipe.production, inFlightConfig: s.productionConfig, part: s.live.part + 1, cycleId: "demo-cycle-" + (s.live.part + 1) } };
    }
    case "live-stop": return { ...s, live: { ...s.live, accepting: false } };
    case "live-step": {
      if (!working(s)) return s;
      const finish = s.live.phase === 3;
      return { ...s, productionConfig: finish && s.live.queuedConfig ? s.live.queuedConfig : s.productionConfig, recipe: finish && s.live.queued !== null ? { ...s.recipe, production: s.live.queued } : s.recipe, live: { ...s.live, accepting: finish && !s.live.continuous ? false : s.live.accepting, phase: s.live.phase + 1, result: finish ? s.live.scenario : null, queued: finish ? null : s.live.queued, queuedConfig: finish ? null : s.live.queuedConfig } };
    }
    case "live-option": return { ...s, live: { ...s.live, ...a.patch, scenario: working(s) ? s.live.scenario : a.patch.scenario ?? s.live.scenario } };
    case "record": return historyRecords.some(r => r.id === a.id) ? { ...s, record: a.id } : s;
    case "compare": {
      const record = historyRecords.find(r => r.id === s.record), mode = a.mode ?? "candidate";
      if (!record || a.kind === "remeasure" && !record.raw || a.kind === "rejudge" && !layoutCompatible(s, record.config)) return s;
      if (a.kind === "remeasure" && mode === "candidate" && s.frames.some((f, i) => f.camera !== record.config.shots[i].camera || f.view !== record.config.shots[i].view || f.shotId !== record.config.shots[i].shotId || f.poseId !== record.config.shots[i].poseId)) return s;
      const config = mode === "original" ? record.config : runtimeConfig(s.recipe, s.frames, s.overview, s.recipe.candidate);
      const invalid = mode === "candidate" && s.frames.some(f => shotProblem(s, f));
      const verdict: Verdict = record.result === "ERR" || invalid ? "ERR" : mode === "original" ? record.result : record.gap > (config.shots[2].params.gapLimit ?? config.recipe.maxGap) || Math.abs(0.3 - config.recipe.target) > config.recipe.tolerance || config.shots[2].params.minWidth > 3.94 || config.shots[2].params.maxWidth < 3.94 ? "NG" : "OK";
      return { ...s, comparisons: { ...s.comparisons, [record.id]: { kind: a.kind, mode, verdict, gap: record.gap, version: config.version, cycleId: record.cycleId, bundleHash: mode === "original" ? record.bundleHash : config.bundleHash } } };
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
    if (id === "teach-history") { f.backup = structuredClone({ imageId: f.imageId, source: f.source, sourceRecord: f.sourceRecord, quality: f.quality, captureSettings: f.captureSettings, params: f.params, path: f.path, mmPerPx: f.mmPerPx }); f.imageId = 184003; f.source = "history"; f.sourceRecord = "TJ-000184"; }
  }
  if (id === "coverage-fail") f.path[0] = [-20, 200];
  if (id === "calibration-fail") s.calibration = { captured: true, result: "fail", saved: false, sample: "bad", version: 7 };
  if (id.startsWith("validation-") || id === "publish-confirm") {
    if (id !== "validation-blocked") { f.trial = { imageId: f.imageId!, revision: f.revision, pass: true, score: 0.94 }; f.saved = true; }
    if (id === "validation-pass" || id === "validation-released" || id === "publish-confirm") s.validation = { status: "passed", revision: s.recipe.revision };
    if (id === "validation-released") { s.recipe.production = 14; s.productionConfig = runtimeConfig(s.recipe, s.frames, s.overview, 14); }
  }
  if (id.startsWith("live-")) {
    s.live.inFlightConfig = s.productionConfig; s.live.cycleId = "demo-cycle-184";
    if (id === "live-running") { s.live.phase = 2; s.live.accepting = true; s.live.inFlightVersion = 13; }
    if (id === "live-ng" || id === "live-k4" || id === "live-err") { s.live.phase = 4; s.live.inFlightVersion = 13; s.live.result = id === "live-err" ? "ERR" : "NG"; s.live.scenario = s.live.result; }
    if (id === "live-k4") s.selectedFrame = 4;
  }
  if (id === "history-no-raw") s.record = "TJ-000182";
  if (id === "history-compare") s.comparisons["TJ-000184"] = { kind: "remeasure", mode: "original", verdict: "NG", gap: 6.2, version: 13, cycleId: "demo-cycle-184", bundleHash: originalConfig.bundleHash };
  return { ...s, recipeLibrary: s.recipeLibrary.map(entry => entry.recipe.name === s.recipe.name ? workspaceOf(s) : entry) };
}
type StoredObject = Record<string, unknown>;
const object = (value: unknown): value is StoredObject => value !== null && typeof value === "object" && !Array.isArray(value);
const numbers = (value: unknown, keys: string[]) => object(value) && keys.every(key => typeof value[key] === "number" && Number.isFinite(value[key]));
const booleans = (value: unknown, keys: string[]) => object(value) && keys.every(key => typeof value[key] === "boolean");
const verdict = (value: unknown) => value === "OK" || value === "NG" || value === "ERR";
const nullableNumber = (value: unknown) => value === null || typeof value === "number" && Number.isFinite(value);
const validParams = (value: unknown) => numbers(value, ["search", "minWidth", "maxWidth"]) && ["dark", "light"].includes(String((value as StoredObject).polarity)) && nullableNumber((value as StoredObject).gapLimit);
const validRecipe = (value: unknown) => object(value) && typeof value.id === "string" && typeof value.name === "string" && Boolean(value.name.trim()) && numbers(value, ["productCode", "candidate", "production", "revision", "spacing", "target", "tolerance", "maxGap"]);
const validPath = (value: unknown) => Array.isArray(value) && value.length <= 1000 && value.every(p => Array.isArray(p) && p.length === 2 && p.every(n => typeof n === "number" && Number.isFinite(n)));
const validCamera = (value: unknown) => object(value) && typeof value.id === "string" && booleans(value, ["connected", "applied"]) && numbers(value, ["exposure", "gain"]) && typeof value.trigger === "string" && [1, 3].includes(Number(value.viewCount)) && Array.isArray(value.size) && value.size.length === 2 && value.size.every(n => Number.isInteger(n) && Number(n) > 0);
const validCalibration = (value: unknown) => object(value) && booleans(value, ["captured", "saved"]) && numbers(value, ["version"]) && ["idle", "pass", "fail"].includes(String(value.result)) && ["good", "bad"].includes(String(value.sample));
function validOverview(value: unknown): boolean {
  return object(value) && typeof value.saved === "boolean" && (value.background === null || typeof value.background === "string" && /^data:image\/(png|jpeg|webp);/.test(value.background)) && Array.isArray(value.positions) && value.positions.length === 6 && value.positions.every(p => numbers(p, ["x", "y"]));
}
function validShot(value: unknown, index: number): boolean {
  return object(value) && value.id === index + 1 && ["shotId", "poseId", "camera"].every(k => typeof value[k] === "string") && numbers(value, ["view", "mmPerPx"]) && typeof value.skip === "boolean" && validPath(value.path) && Array.isArray(value.size) && value.size.length === 2 && value.size.every(n => Number.isInteger(n) && Number(n) > 0) && validParams(value.params) && nullableNumber(value.imageId) && numbers(value.captureSettings, ["exposure"]) && typeof (value.captureSettings as StoredObject).calibrationVersion === "string";
}
function validRuntime(value: unknown): boolean {
  return object(value) && numbers(value, ["version"]) && validRecipe(value.recipe) && Array.isArray(value.shots) && value.shots.length === 6 && value.shots.every(validShot) && ["bundleHash", "recipeHash", "graphVersion", "engineVersion", "calibrationVersion"].every(k => typeof value[k] === "string") && validOverview(value.overview);
}
function validFrame(value: unknown, index: number): boolean {
  if (!validShot(value, index) || !object(value) || !numbers(value, ["revision"]) || typeof value.saved !== "boolean" || !["camera", "history"].includes(String(value.source)) || !["normal", "low"].includes(String(value.quality)) || value.sourceRecord !== null && typeof value.sourceRecord !== "string") return false;
  if (value.trial !== null && (!numbers(value.trial, ["imageId", "revision", "score"]) || !booleans(value.trial, ["pass"]))) return false;
  const backup = value.backup;
  return backup === null || object(backup) && nullableNumber(backup.imageId) && validParams(backup.params) && validPath(backup.path) && numbers(backup, ["mmPerPx"]) && ["camera", "history"].includes(String(backup.source)) && ["normal", "low"].includes(String(backup.quality)) && (backup.sourceRecord === null || typeof backup.sourceRecord === "string") && numbers(backup.captureSettings, ["exposure"]) && typeof (backup.captureSettings as StoredObject).calibrationVersion === "string";
}
function validWorkspace(value: unknown): boolean {
  return object(value) && validRecipe(value.recipe) && validRuntime(value.productionConfig) && Array.isArray(value.frames) && value.frames.length === 6 && value.frames.every(validFrame) && validOverview(value.overview) && object(value.validation) && ["idle", "passed", "failed"].includes(String(value.validation.status)) && nullableNumber(value.validation.revision);
}
export function restorePreview(value: unknown): WorkflowState {
  const fallback = initialState();
  if (!object(value) || value.schema !== 4 || !validWorkspace(value) || typeof value.scene !== "string" || !Number.isInteger(value.selectedFrame) || Number(value.selectedFrame) < 1 || Number(value.selectedFrame) > 6 || !numbers(value, ["imageSequence"])) return fallback;
  if (!validCamera(value.device) || !Array.isArray(value.cameras) || value.cameras.length !== 3 || !value.cameras.every(validCamera) || !validCalibration(value.calibration) || !object(value.calibrations) || !value.cameras.every(c => validCalibration((value.calibrations as StoredObject)[c.id]))) return fallback;
  if (!object(value.plc) || !booleans(value.plc, ["connected", "ready", "pointsApplied"]) || typeof value.plc.address !== "string" || !["S7", "Modbus TCP"].includes(String(value.plc.protocol)) || !Array.isArray(value.plc.points) || value.plc.points.length !== 6 || !value.plc.points.every(p => typeof p === "string")) return fallback;
  if (!object(value.live) || !booleans(value.live, ["accepting", "auto", "continuous"]) || !Number.isInteger(value.live.phase) || Number(value.live.phase) < 0 || Number(value.live.phase) > 4 || !numbers(value.live, ["part"]) || !verdict(value.live.scenario) || value.live.result !== null && !verdict(value.live.result) || !nullableNumber(value.live.inFlightVersion) || !nullableNumber(value.live.queued) || value.live.inFlightConfig !== null && !validRuntime(value.live.inFlightConfig) || value.live.queuedConfig !== null && !validRuntime(value.live.queuedConfig) || value.live.cycleId !== null && typeof value.live.cycleId !== "string") return fallback;
  if (!numbers(value.settings, ["retention", "timeout"]) || !booleans(value.settings, ["saved"]) || typeof (value.settings as StoredObject).raw !== "string" || typeof value.record !== "string" || !object(value.comparisons) || !Object.values(value.comparisons).every(c => object(c) && ["remeasure", "rejudge"].includes(String(c.kind)) && ["original", "candidate"].includes(String(c.mode)) && verdict(c.verdict) && numbers(c, ["gap", "version"]) && typeof c.cycleId === "string" && typeof c.bundleHash === "string")) return fallback;
  if (!Array.isArray(value.recipeLibrary) || !value.recipeLibrary.length || !value.recipeLibrary.every(validWorkspace)) return fallback;
  const state = value as unknown as WorkflowState;
  const names = state.recipeLibrary.map(entry => entry.recipe.name);
  if (new Set(names).size !== names.length || !names.includes(state.recipe.name)) return fallback;
  return { ...state, recipeLibrary: state.recipeLibrary.map(entry => entry.recipe.name === state.recipe.name ? workspaceOf(state) : entry) };
}
