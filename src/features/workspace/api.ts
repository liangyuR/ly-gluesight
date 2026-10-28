import { desktopCall } from "../../lib/desktop";
import type { RecipeDoc } from "../cycle/types";
import type { CalibrationFrameView, CaptureRoundV2, FrameMetrics, PathSegment, SkipEnd, SkipSuggestView, TeachPreviewSegment } from "../plan/types";
import { decodeThumbStrip } from "./workbenchDraft";
import type { AlignSuggestion, CalibrationSyncResult, Check, Comparison, FrozenImage, GrayImage, HistoryCandidate, HistoryPick, Sample, Workspace, WorkspaceView } from "./types";

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
  captureGet: (roundId?: string) => desktopCall<CaptureRoundV2 | null>("recipe_capture_get", { roundId }),
  list: () => desktopCall<Workspace[]>("workspace_list"),
  get: (id: string) => desktopCall<WorkspaceView>("workspace_get", { id }),
  create: (doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_create", { doc }),
  remove: (id: string) => desktopCall<void>("workspace_delete", { id }),
  saveDoc: (id: string, revision: number, doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_save_doc", { id, revision, doc }),
  image: (id: string, imageId: string) => desktopCall<ArrayBuffer>("workspace_image", { id, imageId }).then(decodeGray),
  validate: (id: string, revision: number, samples: Sample[], sourceConfirmed = false) => desktopCall<WorkspaceView>("workspace_validate", { id, revision, samples, sourceConfirmed }),
  retryPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_retry_publish", { id, revision }),
  abandonPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_abandon_publish", { id, revision }),
  /** 导入件（轮次）里某台相机第 seq 帧（帧组序号，从 1 起）的原图；缺帧时后端报错。 */
  captureFrameImage: (roundId: string, cameraId: string, seq: number) => desktopCall<ArrayBuffer>("recipe_capture_frame_image", { roundId, cameraId, seq }).then(decodeGray),
  /** 历史件选择器：本配方最近的真机录制件、能否选用与是否已被本候选使用（只读） */
  historyCandidates: (id: string) => desktopCall<HistoryCandidate[]>("workspace_history_candidates", { id }),
  /** 导入这件并设为示教件；按 RB-2 / RB-8 清空图像派生内容，保留段列表结构 */
  setTeachingPiece: (id: string, revision: number, recordId: number) => desktopCall<WorkspaceView>("workspace_set_teaching_piece", { id, revision, recordId }),
  /** 导入件一台相机全部帧组的缩略图条带（分段标定工作台、边界复核） */
  captureRoundThumbs: (roundId: string, cameraId: string) => desktopCall<ArrayBuffer>("recipe_capture_round_thumbs", { roundId, cameraId }).then(decodeThumbStrip),
  /** 把示教件的边界按图像对齐到一件（只读建议，D-SB-51） */
  captureReviewSuggest: (id: string, round: string) => desktopCall<AlignSuggestion>("workspace_capture_review_suggest", { id, round }),
  /** 复核确认一件的边界：保存并确认，原子写入；确认后再同步标定件 */
  captureReviewConfirmCuts: (id: string, revision: number, round: string, cuts: number[]) =>
    desktopCall<WorkspaceView>("workspace_capture_review_confirm_cuts", { id, revision, round, cuts }),
  /** 工作台一次保存：段列表 + 示教件边界（确认） */
  segmentTemplateSave: (id: string, revision: number, segments: PathSegment[], cuts: number[]) =>
    desktopCall<WorkspaceView>("workspace_segment_template_save", { id, revision, segments, cuts }),
  publish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_publish", { id, revision }),
  /** 采集版检查：计划相机、相机组、曝光与增益、PLC 型号、段、生产版本；不改候选 */
  collectChecks: (id: string, revision: number) => desktopCall<Check[]>("workspace_collect_checks", { id, revision }),
  /** 发布生产配方前检查（设备、走廊示教、段标定、标定采集参数、x_ref、生产版本），当场核对，不改候选（D-SB-46） */
  publishChecks: (id: string, revision: number) => desktopCall<Check[]>("workspace_publish_checks", { id, revision }),
  /** 发布采集版（只供维护模式采图，不判定），在工件边界生效 */
  publishCollect: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_publish_collect", { id, revision }),
  compare: (id: string, revision: number, historyId: number, raw: boolean) =>
    desktopCall<Comparison>("workspace_compare", { id, revision, historyId, raw }),
  compareOriginal: (historyId: number) => desktopCall<Comparison>("workspace_compare_original", { historyId }),
  comparisons: (id:string,historyId:number)=>desktopCall<Comparison[]>("workspace_comparisons",{id,historyId}),
  stationCapture: (cam:number,view=1)=>desktopCall<FrozenImage>("workspace_station_capture",{cam,view}),
  stationImport: (cam:number,bytes:number[],view=1)=>desktopCall<FrozenImage>("workspace_station_import",{cam,bytes,view}),
  stationImage: (cam:number,imageId:string,view=1)=>desktopCall<ArrayBuffer>("workspace_station_image",{cam,imageId,view}).then(decodeGray),

  // ───── 段配方（分段连拍飞拍）：命令与参数见 docs/decisions/segment-burst.md 5.4，结果都在 WorkspaceView.plan ─────
  /** 在这台相机某段冻结的示教帧 imageId 上按选区提取走廊 */
  extractCorridor: (id: string, revision: number, camera: string, imageId: string, roi: [number, number, number, number], guide?: import("./teachingPath").BrushGuide, pointSpacingPx = 20) =>
    desktopCall<{path: [number, number][]; message: string; failure?: { point: [number, number]; reason: string; message: string } | null}>("workspace_extract_corridor", { id, revision, camera, imageId, roi, guide, pointSpacingPx }),
  /** 整体替换候选的段列表。后端拒绝直接改 expectedFrames / calib；段序列或类型变化时清空全部 N_exp、标定与 xRef */
  updateSegments: (id: string, revision: number, segments: PathSegment[]) =>
    desktopCall<WorkspaceView>("workspace_update_segments", { id, revision, segments }),
  /** 段走廊的示教帧（D-SB-45）：只取自示教件，帧组落在该段内；相机还没有像素当量时后端用设备标定的当量填上 */
  teachSegmentCorridor: (id: string, revision: number, camera: string, segment: number, round: string, seq: number) =>
    desktopCall<WorkspaceView>("workspace_teach_segment_corridor", { id, revision, camera, segment, round, seq }),
  /** 去掉一台相机在某段的走廊与示教帧，这一段回到待示教 */
  clearSegmentCorridor: (id: string, revision: number, camera: string, segment: number) =>
    desktopCall<WorkspaceView>("workspace_clear_segment_corridor", { id, revision, camera, segment }),
  /** 选标定件：导入所选历史件（可标标准件），按示教件边界对齐，可信则自动确认并加入标定，否则待复核 */
  calibrationPick: (id: string, revision: number, picks: HistoryPick[]) =>
    desktopCall<CalibrationSyncResult>("workspace_calibration_pick", { id, revision, picks }),
  /** 同步标定件：未确认边界的件对齐、可信则自动确认，已确认的测量并加入标定；示教件在走廊示教完成后加入 */
  calibrationSync: (id: string, revision: number) => desktopCall<CalibrationSyncResult>("workspace_calibration_sync", { id, revision }),
  /** 弃用一件（示教件不能弃用） */
  calibrationRemoveRound: (id: string, revision: number, round: string) =>
    desktopCall<WorkspaceView>("workspace_calibration_remove_round", { id, revision, round }),
  captureReviewExclude: (id: string, revision: number, round: string, start: number, end: number, excluded: boolean, reason: string) =>
    desktopCall<WorkspaceView>("workspace_capture_review_exclude", { id, revision, round, start, end, excluded, reason }),
  /** 按已加入的轮次算 N_exp / 整件 ΣN_exp 与建议容差，结果在 plan.calibration；建议容差要再经 confirmTolerance 才写入 */
  /** 已加入标定的一轮的每帧特征（加入标定时测的），只读 */
  calibrationRoundFeatures: (id: string, round: string) => desktopCall<FrameMetrics[]>("workspace_calibration_round_features", { id, round }),
  /** 用当前段走廊重测标定轮次的一帧，只读 */
  calibrationFrame: (id: string, round: string, camera: string, seq: number) => desktopCall<CalibrationFrameView>("workspace_calibration_frame", { id, round, camera, seq }),
  teachPreviewFrame: (id: string, camera: string, segment: number, round: string, seq: number) => desktopCall<CalibrationFrameView>("workspace_teach_preview_frame", { id, camera, segment, round, seq }),
  teachPreviewSegment: (id: string, camera: string, segment: number, round: string) => desktopCall<TeachPreviewSegment>("workspace_teach_preview_segment", { id, camera, segment, round }),
  calibrationApply: (id: string, revision: number) =>
    desktopCall<WorkspaceView>("workspace_calibration_apply", { id, revision }),
  /** 操作员确认第 segment 段（PathSegment.index）的帧数容差 */
  confirmTolerance: (id: string, revision: number, segment: number, tolerance: number) =>
    desktopCall<WorkspaceView>("workspace_confirm_tolerance", { id, revision, segment, tolerance }),
  /** 第 1 段起涂、末段收尾跳过帧的建议（D-SB-50），由标定轮次的每帧特征算出，只读 */
  calibrationSkipSuggest: (id: string) => desktopCall<SkipSuggestView[]>("workspace_calibration_skip_suggest", { id }),
  /** 操作员确认第 segment 段一头的跳过帧组数 */
  confirmSkip: (id: string, revision: number, segment: number, end: SkipEnd, frames: number) =>
    desktopCall<WorkspaceView>("workspace_confirm_skip", { id, revision, segment, end, frames }),
};
