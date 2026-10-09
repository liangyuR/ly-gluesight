import { vi } from "vitest";
import type { RecipeDoc, RecipeSummary, ShotSpec, Snapshot } from "../src/features/cycle/types";
import type { PartDetail, PartSummary } from "../src/features/history/types";
import type { WorkspaceView } from "../src/features/workspace/types";
import type { useWorkspace } from "../src/features/workspace/context";

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

/** 一台相机按顺序拍这些位置：编号 P1、P2…，Pose 同编号（与 recipe.rs 的 shot_list 一致）。 */
export function shotList(centers: [number, number][], camera = "CAM-1"): ShotSpec[] {
  return centers.map((center, k) => ({ id: `P${k + 1}`, poseId: `P${k + 1}`, camera, center }));
}

export function workspaceView(id = "A"): WorkspaceView {
  const position = { nominal: 3, tolUpper: 1, tolLower: 1, absMin: 1, absMax: 6, maxExcursionLen: 2 };
  const limits = { position, width: null };
  const doc: RecipeDoc = {
    id, name: `工件 ${id}`, version: 2, productCode: 1, triggerMode: "fly", schemaVersion: 2,
    path: { kind: "roundedRect", width: 100, height: 60, radius: 5 },
    spacing: 1, filterWindow: 3, maxGapLen: .5, line: limits, corner: structuredClone(limits),
    segmentOverrides: {}, fov: [120, 80], shots: shotList([[25, 30], [75, 30]]),
  };
  const layout = {
    ...structuredClone(doc), hash: `hash-${id}`, part: [100, 60, 5] as [number, number, number], closed: true,
    segments: [{ name: "直边", kind: "line" as const, s0: 0, s1: 4, params: position, width: null }],
    points: { x: [0, 25, 50, 75], y: [0, 0, 0, 0], seg: [0, 0, 0, 0], k: [0, 0, 1, 1] },
  };
  return {
    layout, productionVersion: 1, coverage: 100,
    workspace: {
      doc, baseHash: `production-${id}`, revision: 7, updatedAt: 1, publishError: null, pending: null,
      frames: [0, 1].map(k => ({
        k, saved: false, backup: null,
        params: { rect: [10, 10, 32, 32], dx: 0, dy: 0, deg: 0, mmPerPx: .1, searchMm: 4, minContrast: 32, minScore: .8 },
        image: { id: `${id}-image-${k}`, source: "camera", capturedAt: 1, size: [100, 60], camera: "CAM-1",
          cameraTag: "cam-v1", calibTag: "calib-v1", geometryTag: "geom-v1", exposureUs: 60, gainDb: 6, historyId: null },
        trial: { imageId: `${id}-image-${k}`, paramsTag: "params-v1", geometryTag: "geom-v1", passed: true,
          score: .94, coverage: 1, elapsedMs: 8, reason: "试测通过", measurement: null },
      })),
      overview: { background: null, positions: [[.25, .5], [.75, .5]], saved: true },
      samples: [{ historyId: null, sampleId: "good", expected: "OK" }, { historyId: null, sampleId: "bad", expected: "NG_GAP" }],
      sampleBank: [
        { id: "good", name: "良品组", geometryTag: "geom-v1", expected: "OK", createdAt: 1 },
        { id: "bad", name: "断胶组", geometryTag: "geom-v1", expected: "NG_GAP", createdAt: 1 },
      ],
      validation: { revision: 7, passed: true, checkedAt: 1, environmentTag: "env-v1", checks: [], samples: [] },
    },
  };
}

export function summary(view = workspaceView()): RecipeSummary {
  const { layout: r } = view;
  return { id: r.id, name: r.name, version: 1, hash: r.hash, productCode: r.productCode,
    shotCount: r.shots.length, triggerMode: r.triggerMode, cameras: [...new Set(r.shots.map(s => s.camera))], length: 4 };
}

export function workspaceState(view = workspaceView()): ReturnType<typeof useWorkspace> {
  return {
    list: [summary(view)], drafts: [view.workspace], cameras: [], selectedId: view.workspace.doc.id,
    data: view, doc: view.workspace.doc, preview: view.layout, previewError: "", error: "", notice: "",
    busy: false, dirty: false, frameDirty: false, frameDrafts: {},
    select: vi.fn(async () => view), clearSelection: vi.fn(), reloadList: vi.fn(async () => {}),
    setDoc: vi.fn(), setFrameParams: vi.fn(), setError: vi.fn(),
    saveDoc: vi.fn(async () => view), act: vi.fn(async request => request()),
  };
}

export function snapshot(phase: Snapshot["phase"] = "IDLE"): Snapshot {
  return {
    phase, since: 1, fault: null, productSource: "manual", activeRecipeId: "A", triggerMode: "fly",
    part: null, result: null, stats: { total: 0, ok: 0, ng: 0, err: 0 }, strayFrames: 0, alarms: [],
  };
}

export function partSummary(id = 1): PartSummary {
  return { id, sn: 100 + id, ts: 1, recipeId: "A", recipeVersion: 1, recipeHash: "hash-A", triggerMode: "fly",
    verdict: "NG_GAP", plcCode: 2, faultCode: 0, reason: "断胶", drainMs: 20, framesExpected: 2, framesReceived: 2, retestOf: null };
}

export function partDetail(): PartDetail {
  return {
    summary: partSummary(), judgement: { verdict: "NG_GAP", plcCode: 2, faultCode: 0, reason: "断胶", segments: [], gaps: [] },
    frames: [], triggers: 2, softwareVersion: "test", points: { d: [3, 3, 3, 3], st: [0, 0, 0, 0], w: [null, null, null, null] }, retests: [],
  };
}
