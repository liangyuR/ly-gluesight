import { desktopCall } from "../../lib/desktop";
import type { RecipeDoc } from "../cycle/types";
import type { CaptureRound, Comparison, FrozenImage, GrayImage, Overview, RecordImages, Sample, ShotTeach, Workspace, WorkspaceView } from "./types";

export async function decodeGray(input: ArrayBuffer | number[]): Promise<GrayImage> {
  const buf=input instanceof ArrayBuffer?input:Uint8Array.from(input).buffer;
  if (buf.byteLength < 16) throw new Error("原图预览数据为空");
  const view = new DataView(buf);
  const [w, h, width, height] = [0, 4, 8, 12].map(o => view.getUint32(o, true));
  if (!w || !h || w * h > 40_000_000 || buf.byteLength < 16 + w * h) throw new Error("原图预览数据不完整");
  const bytes = new Uint8Array(buf, 16);
  const data = new ImageData(w, h);
  for (let j = 0; j < w * h; j++) {
    data.data[j * 4] = data.data[j * 4 + 1] = data.data[j * 4 + 2] = bytes[j];
    data.data[j * 4 + 3] = 255;
  }
  const canvas = document.createElement("canvas");
  canvas.width = w; canvas.height = h;
  canvas.getContext("2d")!.putImageData(data, 0, 0);
  return { url: canvas.toDataURL("image/png"), width, height };
}

export const workspaceApi = {
  captureStart: (recipeId: string, cameraId: string, plannedCount: number, drainTimeoutMs: number) => desktopCall<CaptureRound>("recipe_capture_start", { recipeId, cameraId, plannedCount, drainTimeoutMs }),
  captureGet: (roundId?: string) => desktopCall<CaptureRound | null>("recipe_capture_get", { roundId }),
  captureStop: () => desktopCall<CaptureRound | null>("recipe_capture_stop"),
  adoptCapture: (id: string, revision: number, roundId: string, reuseTeaching: boolean, correspondenceConfirmed: boolean) => desktopCall<WorkspaceView>("workspace_adopt_capture", { id, revision, roundId, reuseTeaching, correspondenceConfirmed }),
  captureSample: (id: string, revision: number, roundId: string, expected: import("../cycle/types").Verdict, correspondenceConfirmed: boolean) => desktopCall<WorkspaceView>("workspace_capture_sample", { id, revision, roundId, expected, correspondenceConfirmed }),
  setViews: (id: string, revision: number, k: number, views: number[], skip: boolean) => desktopCall<WorkspaceView>("workspace_set_views", { id, revision, k, views, skip }),
  saveDraft: (id: string, revision: number, k: number, params: ShotTeach) => desktopCall<WorkspaceView>("workspace_save_draft", { id, revision, k, params }),
  progress: (id: string, k: number, view: number) => desktopCall<WorkspaceView>("workspace_progress", { id, k, view }),
  extractCenterline: (id: string, revision: number, k: number, imageId: string, roi: [number, number, number, number]) => desktopCall<{path: [number, number][]; message: string}>("workspace_extract_centerline", { id, revision, k, imageId, roi }),
  checkCalibration: (id: string, revision: number, k: number, referenceMm: number, measuredMm: number, reuseMatching = false) => desktopCall<WorkspaceView>("workspace_check_calibration", { id, revision, k, referenceMm, measuredMm, reuseMatching }),
  list: () => desktopCall<Workspace[]>("workspace_list"),
  get: (id: string) => desktopCall<WorkspaceView>("workspace_get", { id }),
  create: (doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_create", { doc }),
  remove: (id: string) => desktopCall<void>("workspace_delete", { id }),
  saveDoc: (id: string, revision: number, doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_save_doc", { id, revision, doc }),
  capture: (id: string, revision: number, k: number) => desktopCall<WorkspaceView>("workspace_capture", { id, revision, k }),
  selectView: (id: string, revision: number, k: number, view: number) => desktopCall<WorkspaceView>("workspace_select_view", { id, revision, k, view }),
  importImage: (id: string, revision: number, k: number, bytes: number[]) => desktopCall<WorkspaceView>("workspace_import_image", { id, revision, k, bytes }),
  image: (id: string, imageId: string) => desktopCall<ArrayBuffer>("workspace_image", { id, imageId }).then(decodeGray),
  /** 按候选里已保存的中线试测冻结原图。 */
  trial: (id: string, revision: number, k: number, imageId: string) =>
    desktopCall<WorkspaceView>("workspace_trial", { id, revision, k, imageId }),
  /** 把中线、像素当量与检测参数写进候选配方的拍照点 k，这一帧的试测随之作废。 */
  saveParams: (id: string, revision: number, k: number, params: ShotTeach) =>
    desktopCall<WorkspaceView>("workspace_save_params", { id, revision, k, params }),
  saveTeach: (id: string, revision: number, k: number, imageId: string) =>
    desktopCall<WorkspaceView>("workspace_save_teach", { id, revision, k, imageId }),
  restoreTeach: (id: string, revision: number, k: number) => desktopCall<WorkspaceView>("workspace_restore_teach", { id, revision, k }),
  saveOverview: (id: string, revision: number, overview: Overview) => desktopCall<WorkspaceView>("workspace_save_overview", { id, revision, overview }),
  validate: (id: string, revision: number, samples: Sample[], sourceConfirmed = false) => desktopCall<WorkspaceView>("workspace_validate", { id, revision, samples, sourceConfirmed }),
  importSample: (id:string, revision:number, name:string, expected:import("../cycle/types").Verdict, images:{k:number;bytes:string}[]) =>
    desktopCall<WorkspaceView>("workspace_import_sample", {id,revision,name,expected,images}),
  retryPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_retry_publish", { id, revision }),
  captureImage: (roundId: string, k: number, view?: number) => desktopCall<ArrayBuffer>("recipe_capture_image", { roundId, k, view }).then(decodeGray),
  captureList: (recipeId: string) => desktopCall<Pick<CaptureRound,"roundId"|"recipeId"|"cameraId"|"plannedCount"|"receivedCount"|"state"|"createdAt"|"error">[]>("recipe_capture_list", { recipeId }),
  publish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_publish", { id, revision }),
  recordImages: (historyId: number) => desktopCall<RecordImages>("workspace_record_images", { historyId }),
  recordImage: (historyId: number, k: number, view?: number) => desktopCall<ArrayBuffer>("workspace_record_image", { historyId, k, view }).then(decodeGray),
  historyCapture: (id: string, revision: number, historyId: number, k: number) =>
    desktopCall<WorkspaceView>("workspace_history_capture", { id, revision, historyId, k }),
  compare: (id: string, revision: number, historyId: number, raw: boolean) =>
    desktopCall<Comparison>("workspace_compare", { id, revision, historyId, raw }),
  compareOriginal: (historyId: number) => desktopCall<Comparison>("workspace_compare_original", { historyId }),
  comparisons: (id:string,historyId:number)=>desktopCall<Comparison[]>("workspace_comparisons",{id,historyId}),
  runtimeOverview: (id: string, revisionId: string) => desktopCall<Overview | null>("workspace_runtime_overview", { id, revisionId }),
  liveImage: (cycleId:string, revisionId:string, k:number) => desktopCall<ArrayBuffer>("workspace_live_image", {cycleId,revisionId,k}).then(decodeGray),
  stationCapture: (cam:number,view=1)=>desktopCall<FrozenImage>("workspace_station_capture",{cam,view}),
  stationImport: (cam:number,bytes:number[],view=1)=>desktopCall<FrozenImage>("workspace_station_import",{cam,bytes,view}),
  stationImage: (cam:number,imageId:string,view=1)=>desktopCall<ArrayBuffer>("workspace_station_image",{cam,imageId,view}).then(decodeGray),
};
