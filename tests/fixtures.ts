import { vi } from "vitest";
import type { DetectParams, Recipe, RecipeDoc, RecipeSummary, ShotLimits, ShotSpec, Snapshot } from "../src/features/cycle/types";
import type { PartDetail, PartSummary } from "../src/features/history/types";
import type { FrozenImage, WorkspaceView } from "../src/features/workspace/types";
import type { useWorkspace } from "../src/features/workspace/context";

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

/** 一台相机按顺序拍这些拍照点：编号 P1、P2…，Pose 同编号，胶条 J1；给了中线时像素当量 0.1 mm/px（与 recipe.rs 的 shot_list 一致）。 */
export function shotList(paths: [number, number][][], camera = "CAM-1"): ShotSpec[] {
  return paths.map((path, k) => ({ id: `P${k + 1}`, poseId: `P${k + 1}`, camera, view: 1, bead: "J1", skip: false, path, ...(path.length ? { mmPerPx: .1 } : {}) }));
}

/** 两个拍照点各一条 10 px（1 mm）的中线，站距 1 mm：每个拍照点两站，自成一段。 */
export const twoLines: [number, number][][] = [[[10, 10], [20, 10]], [[30, 10], [40, 10]]];

export function workspaceView(id = "A"): WorkspaceView {
  const position = { nominal: 3, tolUpper: 1, tolLower: 1, absMin: 1, absMax: 6, maxExcursionLen: 2 };
  const limits: ShotLimits = { position, width: null, maxGapLen: .5, minPresent: .8 };
  const detect: DetectParams = { searchMm: 4, polarity: "dark", widthRange: [1, 6] };
  const doc: RecipeDoc = {
    id, name: `工件 ${id}`, version: 2, productCode: 1, triggerMode: "fly", schemaVersion: 4,
    spacing: 1, filterWindow: 3, detect, limits, shots: shotList(twoLines),
  };
  const layout: Recipe = {
    ...structuredClone(doc), revisionId: `revisionId-${id}`,
    segments: [0, 1].map(k => ({ name: `P${k + 1} · J1`, shot: k, first: 2 * k, count: 2, position: structuredClone(position), width: null, maxGapLen: .5, minPresent: .8 })),
    points: { x: [10, 20, 30, 40], y: [10, 10, 10, 10], seg: [0, 0, 1, 1], k: [0, 0, 1, 1] },
  };
  return {
    layout, productionVersion: 1, productionPlanVersion: 1, planChanged: false, coverage: 100,
    workspace: {
      doc, baseRevision: `production-${id}`, revision: 7, updatedAt: 1, publishError: null, pending: null,
      frames: [0, 1].map(k => {
        const image: FrozenImage = { id: `${id}-image-${k}`, source: "camera", capturedAt: 1, size: [100, 60], camera: "CAM-1", view: 1,
          cameraTag: { id: "CAM-1", source: "sim", viewCount: 1, acquisition: "triggered", exposureUs: 60, gainDb: 6 }, calibTag: { path: "C:/test/calib/plane_calib.json", value: null }, geometryTag: [`P${k + 1}`, `P${k + 1}`, "CAM-1", 1, "CAM-1"], exposureUs: 60, gainDb: 6, historyId: null };
        return { k, saved: false, backup: null, image, views: [image],
        trial: { imageId: `${id}-image-${k}`, paramsTag: [doc.spacing, structuredClone(doc.shots[k].path), doc.shots[k].mmPerPx, structuredClone(detect), doc.filterWindow], geometryTag: [`P${k + 1}`, `P${k + 1}`, "CAM-1", 1, "CAM-1"], engineTag: { path: "C:/test/lyflow_core.dll", version: "1.1.0" }, passed: true,
          score: .94, coverage: 1, elapsedMs: 8, reason: "试测通过", measurement: { ids: [2 * k, 2 * k + 1] } },
        };
      }),
      overview: { background: null, positions: [[.25, .5], [.75, .5]], saved: true },
      samples: [{ historyId: null, sampleId: "good", expected: "OK" }, { historyId: null, sampleId: "bad", expected: "NG_GAP" }],
      sampleBank: [
        { id: "good", name: "良品组", geometryTag: doc.shots.map(shot => [shot.id, shot.poseId, shot.camera, shot.view]), expected: "OK", createdAt: 1 },
        { id: "bad", name: "断胶组", geometryTag: doc.shots.map(shot => [shot.id, shot.poseId, shot.camera, shot.view]), expected: "NG_GAP", createdAt: 1 },
      ],
      validation: { revision: 7, passed: true, checkedAt: 1, environmentTag: { cameras: [{ id: "CAM-1", source: "sim", viewCount: 1 }], calibration: null, engine: { path: "C:/test/lyflow_core.dll", version: "1.1.0" } }, checks: [], samples: [] },
    },
  };
}

export function summary(view = workspaceView()): RecipeSummary {
  const { layout: r } = view;
  return { id: r.id, name: r.name, version: 1, revisionId: r.revisionId, productCode: r.productCode,
    shotCount: r.shots.length, triggerMode: r.triggerMode, cameras: [...new Set(r.shots.map(s => s.camera))], length: 4 };
}

export function tricamWorkspaceView(id = "A"): WorkspaceView {
  const view = workspaceView(id);
  view.workspace.frames.forEach(frame => {
    frame.views = [1, 2, 3].map(value => ({ ...frame.image!, view: value, id: value === 1 ? frame.image!.id : `${frame.image!.id}-v${value}`, geometryTag: [`P${frame.k + 1}`, `P${frame.k + 1}`, "CAM-1", value, "CAM-1"] }));
    frame.image = frame.views[0];
  });
  return view;
}

export function workspaceState(view = workspaceView()): ReturnType<typeof useWorkspace> {
  return {
    list: [summary(view)], drafts: [view.workspace], cameras: [], selectedId: view.workspace.doc.id,
    data: view, doc: view.workspace.doc, preview: view.layout, previewError: "", error: "", notice: "",
    busy: false, dirty: false, frameDirty: false, frameDrafts: {},
    select: vi.fn(async () => view), clearSelection: vi.fn(), reloadList: vi.fn(async () => {}),
    setDoc: vi.fn(), setFrameDraft: vi.fn(), setError: vi.fn(),
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

export function partSummary(id = 1): PartSummary {
  return { id, sn: 100 + id, ts: 1, recipeId: "A", recipeVersion: 1, recipeRevision: "revisionId-A", triggerMode: "fly",
    verdict: "NG_GAP", plcCode: 2, faultCode: 0, reason: "断胶", drainMs: 20, framesExpected: 2, framesReceived: 2, retestOf: null, cycleId: `cycle-${id}`, bundleId: "bundle-A", delivery: { state: "acknowledged", updatedAt: 1, message: null } };
}

export function partDetail(): PartDetail {
  return {
    summary: partSummary(), judgement: { verdict: "NG_GAP", plcCode: 2, faultCode: 0, reason: "断胶", segments: [], gaps: [] },
    shots: [0,1].map(k=>({k,shotId:`P${k+1}`,camera:"CAM-1",view:1,session:1,ordinal:k+1,frameCounter:k+1,triggerCounter:k+1,status:"done",error:null,score:.9,ms:5,rawFiles:[]})),
    recording: {state:"complete",available:true,directory:"recorded",errors:[]},
    frames: [], triggers: 2, softwareVersion: "test", points: { d: [3, 3, 3, 3], st: [0, 0, 0, 0], w: [null, null, null, null] }, retests: [],
  };
}
