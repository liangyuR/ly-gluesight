import { desktopCall } from "../../lib/desktop";
import { decodeGray, liveGray } from "./api";
import type { GrayImage } from "./types";

/**
 * 段配方的原图：按 (相机, seq) 读，camera 是计划相机下标（与 FrameGroupView.cells、FrameRecord.camera 同一口径），不再有拍照点 k 与视角。
 * 决策记录 5.4 的命令清单没有列这两条读图命令：命令名与参数由后端读帧的包（在线 C3、历史 D3）按这里对齐。
 * 旧拍照点配方仍走 workspaceApi.liveImage / recordImage。
 */
export const burstImageApi = {
  /** 在途件某台相机第 seq 帧的实时原图；revisionId 是本件开工时的配方修订 */
  live: (cycleId: string, revisionId: string, camera: number, seq: number): Promise<GrayImage> =>
    desktopCall<ArrayBuffer>("workspace_live_frame_image", { cycleId, revisionId, camera, seq }).then(liveGray),
  /** 历史件某台相机第 seq 帧的录制原图；录制没保存时后端报错 */
  record: (historyId: number, camera: number, seq: number): Promise<GrayImage> =>
    desktopCall<ArrayBuffer>("workspace_record_frame_image", { historyId, camera, seq }).then(decodeGray),
};
