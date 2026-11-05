import { desktopCall } from "../../lib/desktop";
import { decodeFrame, frameImageData } from "../../lib/frame";
import type { NavigationMapData } from "./navigation/types";
import type { RecipeDoc } from "../cycle/types";
import type { CalibrationFrameView, CaptureRoundV2, FrameMetrics, PathSegment, SkipEnd, SkipSuggestView, TeachPreviewSegment } from "../plan/types";
import { decodeThumbStrip } from "./workbenchDraft";
import type { Check, Comparison, FrozenImage, FrameImage, HistoryCandidate, Sample, SegmentSaveImpact, Workspace, WorkspaceView } from "./types";

let liveSeq = 0;

/** 实时帧：保留像素直接画到 canvas，不生成图片地址，避免浏览器把每一帧当作图片资源缓存与解码。 */
export function liveFrame(input: ArrayBuffer | number[]): FrameImage {
  const { fullWidth, fullHeight, pixels } = decodeFrame(input);
  return { url: `live:${++liveSeq}`, width: fullWidth, height: fullHeight, channels: pixels.channels, pixels };
}

export async function decodeFrameImage(input: ArrayBuffer | number[]): Promise<FrameImage> {
  const { fullWidth, fullHeight, pixels } = decodeFrame(input);
  const canvas = document.createElement("canvas");
  canvas.width = pixels.width; canvas.height = pixels.height;
  canvas.getContext("2d")!.putImageData(frameImageData(pixels), 0, 0);
  return { url: canvas.toDataURL("image/png"), width: fullWidth, height: fullHeight, channels: pixels.channels };
}

export const workspaceApi = {
  navigationImport: (id: string, revision: number, bytes: number[]) => desktopCall<WorkspaceView>("workspace_navigation_import", { id, revision, bytes }),
  navigationSave: (id: string, revision: number, map: NavigationMapData, confirm: boolean) => desktopCall<WorkspaceView>("workspace_navigation_save", { id, revision, map, confirm }),
  navigationDelete: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_navigation_delete", { id, revision }),
  navigationImage: (id: string, imageId: string) => desktopCall<ArrayBuffer | number[]>("workspace_navigation_image", { id, imageId }),
  debugRun: (id: string, revision: number, partId: string | null) => desktopCall<void>("workspace_debug_run", { id, revision, partId }),
  captureGet: (roundId?: string) => desktopCall<CaptureRoundV2 | null>("recipe_capture_get", { roundId }),
  list: () => desktopCall<Workspace[]>("workspace_list"),
  get: (id: string) => desktopCall<WorkspaceView>("workspace_get", { id }),
  create: (doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_create", { doc }),
  remove: (id: string) => desktopCall<void>("workspace_delete", { id }),
  saveDoc: (id: string, revision: number, doc: RecipeDoc) => desktopCall<WorkspaceView>("workspace_save_doc", { id, revision, doc }),
  image: (id: string, imageId: string) => desktopCall<ArrayBuffer>("workspace_image", { id, imageId }).then(decodeFrameImage),
  validate: (id: string, revision: number, samples: Sample[], sourceConfirmed = false) => desktopCall<WorkspaceView>("workspace_validate", { id, revision, samples, sourceConfirmed }),
  retryPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_retry_publish", { id, revision }),
  abandonPublish: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_abandon_publish", { id, revision }),
  /** 导入件（轮次）里某台相机第 seq 帧（帧组序号，从 1 起）的原图；缺帧时后端报错。 */
  captureFrameImage: (roundId: string, cameraId: string, seq: number) => desktopCall<ArrayBuffer>("recipe_capture_frame_image", { roundId, cameraId, seq }).then(decodeFrameImage),
  /** 历史件选择器：本配方最近的真机录制件、能否选用与是否已被本候选使用（只读） */
  historyCandidates: (id: string) => desktopCall<HistoryCandidate[]>("workspace_history_candidates", { id }),
  /** 导入这件并设为示教件；按 RB-2 / RB-8 清空图像派生内容，保留段列表结构 */
  setTeachingPiece: (id: string, revision: number, recordId: number) => desktopCall<WorkspaceView>("workspace_set_teaching_piece", { id, revision, recordId }),
  /** 导入件一台相机全部帧组的缩略图条带（分段标定工作台） */
  captureRoundThumbs: (roundId: string, cameraId: string) => desktopCall<ArrayBuffer>("recipe_capture_round_thumbs", { roundId, cameraId }).then(decodeThumbStrip),
  /** 工作台一次保存：段列表 + 示教件边界（确认） */
  segmentTemplateSave: (id: string, revision: number, segments: PathSegment[], cuts: number[]) =>
    desktopCall<WorkspaceView>("workspace_segment_template_save", { id, revision, segments, cuts }),
  /** 工作台保存前预演（只读）：会清掉哪些走廊、保存后哪些段缺走廊 */
  segmentTemplatePreview: (id: string, revision: number, segments: PathSegment[], cuts: number[]) =>
    desktopCall<SegmentSaveImpact>("workspace_segment_template_preview", { id, revision, segments, cuts }),
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
  recordFrameView: (historyId: number, comparison: string, camera: number, seq: number) => desktopCall<CalibrationFrameView>("workspace_record_frame_view", { historyId, comparison, camera, seq }),
  stationCapture: (cam:number,view=1)=>desktopCall<FrozenImage>("workspace_station_capture",{cam,view}),
  stationImport: (cam:number,bytes:number[],view=1)=>desktopCall<FrozenImage>("workspace_station_import",{cam,bytes,view}),
  /** 当前标定所用原图载入为冻结样本；没有可载入的原图为 null */
  stationRestore: (cam:number,view=1)=>desktopCall<FrozenImage|null>("workspace_station_restore",{cam,view}),
  stationImage: (cam:number,imageId:string,view=1)=>desktopCall<ArrayBuffer>("workspace_station_image",{cam,imageId,view}).then(decodeFrameImage),

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
  /** 测量示教件：按当前走廊测每帧特征，得出胶偏参考 x_ref（单段配方）与跳过帧建议的依据 */
  measureTeaching: (id: string, revision: number) => desktopCall<WorkspaceView>("workspace_measure_teaching", { id, revision }),
  /** 屏蔽或恢复示教件的帧组：只排除图像测量，示教件已有的测量随之失效 */
  captureReviewExclude: (id: string, revision: number, round: string, start: number, end: number, excluded: boolean, reason: string) =>
    desktopCall<WorkspaceView>("workspace_capture_review_exclude", { id, revision, round, start, end, excluded, reason }),
  /** 示教件测量时写下的每帧特征，只读 */
  calibrationRoundFeatures: (id: string, round: string) => desktopCall<FrameMetrics[]>("workspace_calibration_round_features", { id, round }),
  /** 用当前段走廊重测示教件的一帧，只读 */
  calibrationFrame: (id: string, round: string, camera: string, seq: number) => desktopCall<CalibrationFrameView>("workspace_calibration_frame", { id, round, camera, seq }),
  teachPreviewFrame: (id: string, camera: string, segment: number, round: string, seq: number) => desktopCall<CalibrationFrameView>("workspace_teach_preview_frame", { id, camera, segment, round, seq }),
  teachPreviewSegment: (id: string, camera: string, segment: number, round: string) => desktopCall<TeachPreviewSegment>("workspace_teach_preview_segment", { id, camera, segment, round }),
  /** 操作员确认第 segment 段（PathSegment.index）的帧数容差 */
  confirmTolerance: (id: string, revision: number, segment: number, tolerance: number) =>
    desktopCall<WorkspaceView>("workspace_confirm_tolerance", { id, revision, segment, tolerance }),
  /** 第 1 段起涂、末段收尾跳过帧的建议（D-SB-50），由示教件测量的每帧特征算出，只读 */
  calibrationSkipSuggest: (id: string) => desktopCall<SkipSuggestView[]>("workspace_calibration_skip_suggest", { id }),
  /** 操作员确认第 segment 段一头的跳过帧组数 */
  confirmSkip: (id: string, revision: number, segment: number, end: SkipEnd, frames: number) =>
    desktopCall<WorkspaceView>("workspace_confirm_skip", { id, revision, segment, end, frames }),
};
