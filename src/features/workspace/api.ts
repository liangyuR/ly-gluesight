import { desktopCall } from "../../lib/desktop";
import type { CameraImaging, RecipeDoc } from "../cycle/types";
import type { CameraTeachDraft, CaptureRoundV2, PathSegment } from "../plan/types";
import type { CaptureRound, CaptureSummary, Comparison, FixtureCheck, FixtureTrial, FrozenImage, GrayImage, ImagingSaved, Overview, RecordImages, RotationConfirmation, Sample, ShotTeach, Workspace, WorkspaceView } from "./types";

function grayPreview(input: ArrayBuffer | number[]) {
  const buf=input instanceof ArrayBuffer?input:Uint8Array.from(input).buffer;
  if (buf.byteLength < 16) throw new Error("原图预览数据为空");
  const view = new DataView(buf);
  const [w, h, width, height] = [0, 4, 8, 12].map(o => view.getUint32(o, true));
  if (!w || !h || w * h > 40_000_000 || buf.byteLength < 16 + w * h) throw new Error("原图预览数据不完整");
  return { w, h, width, height, bytes: new Uint8Array(buf, 16, w * h) };
}

let liveSeq = 0;

/** 实时帧：保留灰度像素直接画到 canvas，不生成图片地址，避免浏览器把每一帧当作图片资源缓存与解码。 */
export function liveGray(input: ArrayBuffer | number[]): GrayImage {
  const { w, h, width, height, bytes } = grayPreview(input);
  return { url: `live:${++liveSeq}`, width, height, pixels: { data: bytes, width: w, height: h } };
}

export async function decodeGray(input: ArrayBuffer | number[]): Promise<GrayImage> {
  const { w, h, width, height, bytes } = grayPreview(input);
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
  debugRun: (id: string, revision: number, partId: string | null) => desktopCall<void>("workspace_debug_run", { id, revision, partId }),
  selectGroupImage: (id: string, revision: number, k: number, view: number) => desktopCall<WorkspaceView>("workspace_select_group_image", { id, revision, k, view }),
  /** 采集轮次：段配方是 CaptureRoundV2（format: 2，D1 起输出），旧后端仍给旧的 CaptureRound；页面用 captureRound.ts 的 roundView 统一读取 */
  captureStart: (recipeId: string, cameraIds: string[], drainTimeoutMs: number) => desktopCall<CaptureRound | CaptureRoundV2>("recipe_capture_start", { recipeId, cameraIds, drainTimeoutMs }),
  captureGet: (roundId?: string) => desktopCall<CaptureRound | CaptureRoundV2 | null>("recipe_capture_get", { roundId }),
  captureStop: () => desktopCall<CaptureRound | CaptureRoundV2 | null>("recipe_capture_stop"),
  adoptCapture: (id: string, revision: number, roundId: string, reuseTeaching: boolean, correspondenceConfirmed: boolean) => desktopCall<WorkspaceView>("workspace_adopt_capture", { id, revision, roundId, reuseTeaching, correspondenceConfirmed }),
  captureSample: (id: string, revision: number, roundId: string, expected: import("../cycle/types").Verdict, correspondenceConfirmed: boolean) => desktopCall<WorkspaceView>("workspace_capture_sample", { id, revision, roundId, expected, correspondenceConfirmed }),
  setViews: (id: string, revision: number, k: number, views: number[], skip: boolean) => desktopCall<WorkspaceView>("workspace_set_views", { id, revision, k, views, skip }),
  saveDraft: (id: string, revision: number, k: number, params: ShotTeach) => desktopCall<WorkspaceView>("workspace_save_draft", { id, revision, k, params }),
  progress: (id: string, k: number, view: number) => desktopCall<WorkspaceView>("workspace_progress", { id, k, view }),
  extractCenterline: (id: string, revision: number, k: number, imageId: string, roi: [number, number, number, number], guide?: import("./teachingPath").BrushGuide, pointSpacingPx = 20) => desktopCall<{path: [number, number][]; message: string; failure?: { point: [number, number]; reason: string; message: string } | null}>("workspace_extract_centerline", { id, revision, k, imageId, roi, guide, pointSpacingPx }),
  checkCalibration: (id: string, revision: number, k: number, referenceMm: number, measuredMm: number, reuseMatching = false) => desktopCall<WorkspaceView>("workspace_check_calibration", { id, revision, k, referenceMm, measuredMm, reuseMatching }),
  list: () => desktopCall<Workspace[]>("workspace_list"),
  get: (id: string) => desktopCall<WorkspaceView>("workspace_get", { id }),
  create: (doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_create", { doc }),
  remove: (id: string) => desktopCall<void>("workspace_delete", { id }),
  saveDoc: (id: string, revision: number, doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_save_doc", { id, revision, doc }),
  /** 按相机写进候选配方，并在设备空闲时分别下发；applyErrors 为各相机下发失败的原因（参数已保存）。 */
  setCameraImaging: (id: string, revision: number, imaging: Record<string, CameraImaging>) => desktopCall<ImagingSaved>("workspace_set_camera_imaging", { id, revision, imaging }),
  selectView: (id: string, revision: number, k: number, view: number) => desktopCall<WorkspaceView>("workspace_select_view", { id, revision, k, view }),
  image: (id: string, imageId: string) => desktopCall<ArrayBuffer>("workspace_image", { id, imageId }).then(decodeGray),
  /** 按候选里已保存的中线试测冻结原图。 */
  trial: (id: string, revision: number, k: number, imageId: string) =>
    desktopCall<WorkspaceView>("workspace_trial", { id, revision, k, imageId }),
  /** 把中线、像素当量与检测参数写进候选配方的拍照点 k，这一帧的试测随之作废。转角调大时必须带 rotation（高级设置里的确认）。 */
  saveParams: (id: string, revision: number, k: number, params: ShotTeach & { rotation?: RotationConfirmation }) =>
    desktopCall<WorkspaceView>("workspace_save_params", { id, revision, k, params }),
  /** fixtureReviewed：特征定位视图须人工确认框内没有喷嘴、胶枪 */
  saveTeach: (id: string, revision: number, k: number, imageId: string, fixtureReviewed?: boolean) =>
    desktopCall<WorkspaceView>("workspace_save_teach", { id, revision, k, imageId, fixtureReviewed }),
  /** 画框时的只读检查：外圈、走廊、纹理、亮像素与杠杆臂，不跑引擎、不改候选 */
  fixtureCheck: (id: string, revision: number, k: number, params: ShotTeach) =>
    desktopCall<FixtureCheck>("workspace_fixture_check", { id, revision, k, params }),
  /** 定位试测：示教图与样本库同一拍照点同一幅图上跑定位与胶偏，只作诊断 */
  fixtureTrial: (id: string, revision: number, k: number) => desktopCall<FixtureTrial>("workspace_fixture_trial", { id, revision, k }),
  /** 采用当前验证给出的两端参数建议（数值只由后端从验证统计取） */
  applyFixtureSuggestion: (id: string, revision: number, k: number, view: number) =>
    desktopCall<WorkspaceView>("workspace_apply_fixture_suggestion", { id, revision, k, view }),
  saveOverview: (id: string, revision: number, overview: Overview) => desktopCall<WorkspaceView>("workspace_save_overview", { id, revision, overview }),
  validate: (id: string, revision: number, samples: Sample[], sourceConfirmed = false) => desktopCall<WorkspaceView>("workspace_validate", { id, revision, samples, sourceConfirmed }),
  importSample: (id:string, revision:number, name:string, expected:import("../cycle/types").Verdict, images:{k:number;bytes:string}[]) =>
    desktopCall<WorkspaceView>("workspace_import_sample", {id,revision,name,expected,images}),
  retryPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_retry_publish", { id, revision }),
  abandonPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_abandon_publish", { id, revision }),
  captureImage: (roundId: string, k: number, cameraId: string, view?: number) => desktopCall<ArrayBuffer>("recipe_capture_image", { roundId, k, cameraId, view }).then(decodeGray),
  /** 新采集轮次（CaptureRoundV2）里某台相机第 seq 帧（帧组序号，从 1 起）的原图；命令名与参数由 D1 在 recipe_capture_image 旁新增，缺帧时后端报错。 */
  captureFrameImage: (roundId: string, cameraId: string, seq: number) => desktopCall<ArrayBuffer>("recipe_capture_frame_image", { roundId, cameraId, seq }).then(decodeGray),
  captureList: (recipeId: string) => desktopCall<CaptureSummary[]>("recipe_capture_list", { recipeId }),
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
  liveImage: (cycleId:string, revisionId:string, k:number) => desktopCall<ArrayBuffer>("workspace_live_image", {cycleId,revisionId,k}).then(liveGray),
  stationCapture: (cam:number,view=1)=>desktopCall<FrozenImage>("workspace_station_capture",{cam,view}),
  stationImport: (cam:number,bytes:number[],view=1)=>desktopCall<FrozenImage>("workspace_station_import",{cam,bytes,view}),
  stationImage: (cam:number,imageId:string,view=1)=>desktopCall<ArrayBuffer>("workspace_station_image",{cam,imageId,view}).then(decodeGray),

  // ───── 段配方（分段连拍飞拍）：命令与参数见 docs/decisions/segment-burst.md 5.4，结果都在 WorkspaceView.plan ─────
  /** 整体替换候选的段列表。后端拒绝直接改 expectedFrames / calib；段序列或类型变化时清空全部 N_exp、标定与 xRef */
  updateSegments: (id: string, revision: number, segments: PathSegment[]) =>
    desktopCall<WorkspaceView>("workspace_update_segments", { id, revision, segments }),
  /** 用采集轮次 round 里这台相机第 seq 帧示教走廊：冻结该帧，按 draft 写进候选配方的 inspect */
  teachCamera: (id: string, revision: number, camera: string, round: string, seq: number, draft: CameraTeachDraft) =>
    desktopCall<WorkspaceView>("workspace_teach_camera", { id, revision, camera, round, seq, draft }),
  /** 把采集轮次加入多轮标定；standard = 标准件轮次（用于胶偏参考 xRef） */
  calibrationAddRound: (id: string, revision: number, round: string, standard: boolean) =>
    desktopCall<WorkspaceView>("workspace_calibration_add_round", { id, revision, round, standard }),
  calibrationRemoveRound: (id: string, revision: number, round: string) =>
    desktopCall<WorkspaceView>("workspace_calibration_remove_round", { id, revision, round }),
  /** 按已加入的轮次算 N_exp / 整件 ΣN_exp 与建议容差，结果在 plan.calibration；建议容差要再经 confirmTolerance 才写入 */
  calibrationApply: (id: string, revision: number) =>
    desktopCall<WorkspaceView>("workspace_calibration_apply", { id, revision }),
  /** 操作员确认第 segment 段（PathSegment.index）的帧数容差 */
  confirmTolerance: (id: string, revision: number, segment: number, tolerance: number) =>
    desktopCall<WorkspaceView>("workspace_confirm_tolerance", { id, revision, segment, tolerance }),
};
