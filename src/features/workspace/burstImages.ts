import { pendingCall } from "../../lib/pendingBackend";
import { decodeGray, liveGray } from "./api";
import type { GrayImage } from "./types";

/**
 * 段配方的原图：按 (相机, seq) 读，camera 是计划相机下标（与 FrameGroupView.cells、FrameRecord.camera 同一口径），不再有拍照点 k 与视角。
 * 命令登记在决策记录 5.4：在线读图归 C3，历史读图归 D3；后端落地前命令不存在，pendingCall 改报“该功能待后端 <包> 实现”，页面显示占位。
 * 旧拍照点配方仍走 workspaceApi.liveImage / recordImage。
 */
export const burstImageApi = {
  /** 在途件某台相机第 seq 帧的实时原图；revisionId 是本件开工时的配方修订 */
  live: (cycleId: string, revisionId: string, camera: number, seq: number): Promise<GrayImage> =>
    pendingCall<ArrayBuffer>("workspace_live_frame_image", "C3", { cycleId, revisionId, camera, seq }).then(liveGray),
  /** 历史件某台相机第 seq 帧的录制原图；录制没保存时后端报错 */
  record: (historyId: number, camera: number, seq: number): Promise<GrayImage> =>
    pendingCall<ArrayBuffer>("workspace_record_frame_image", "D3", { historyId, camera, seq }).then(decodeGray),
};
