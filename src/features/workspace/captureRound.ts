/**
 * 示教采集轮次的统一读取：段配方的新轮次（CaptureRoundV2，按帧号配组、不核对计数）与旧后端仍给的旧轮次（CaptureRound）
 * 都先整理成 RoundView，页面只看帧组数、各相机帧数、缺帧的帧组与丢帧告警，不再按 PLC 计划数核对。
 * 帧组 = 同一触发序号的各相机帧；seq 从 1 起，页面上的“帧组序号”就是它。
 */
import type { CaptureRoundV2, LossAlarm } from "../plan/types";
import { workspaceApi } from "./api";
import type { CaptureRound, GrayImage } from "./types";

export type AnyRound = CaptureRound | CaptureRoundV2;

export const isRoundV2 = (round: AnyRound): round is CaptureRoundV2 => (round as CaptureRoundV2).format === 2;

export interface RoundView {
  roundId: string;
  recipeId: string;
  state: CaptureRound["state"];
  simulated: boolean;
  error: string | null;
  format: 1 | 2;
  /** 按本轮相机顺序；frames 为该相机收到的帧数 */
  cameras: { cameraId: string; frames: number }[];
  /** 帧组数 = 各相机最大 seq 的最大值 */
  groupCount: number;
  /** 缺帧的帧组序号（从 1 起，升序）：组内至少一台相机没有这一帧 */
  incompleteGroups: number[];
  lossAlarms: LossAlarm[];
  /** PLC 写来的预期总数：示教采集只作参考，不核对 */
  plcEstimate: number | null;
  /** PLC 结束时的累计触发数：只作参考 */
  deviceTriggers: number | null;
  /** 哪台相机（本轮相机下标）缺哪个 seq */
  missing: { camera: number; seq: number }[];
}

export function roundView(round: AnyRound): RoundView {
  if (isRoundV2(round)) {
    return {
      roundId: round.roundId, recipeId: round.recipeId, state: round.state, simulated: round.simulated, error: round.error, format: 2,
      cameras: round.cameras.map((camera, i) => ({ cameraId: camera.cameraId, frames: round.cameraFrames[i] ?? camera.receivedCount })),
      groupCount: round.groupCount, incompleteGroups: [...round.incompleteGroups].sort((a, b) => a - b), lossAlarms: round.lossAlarms,
      plcEstimate: round.plcShotCount, deviceTriggers: round.deviceTriggers, missing: round.missingFrames,
    };
  }
  const missing = round.frames.flatMap((frame, k) => round.cameras.flatMap((camera, i) => frame.cameras.some(c => c.cameraId === camera.cameraId) ? [] : [{ camera: i, seq: k + 1 }]));
  return {
    roundId: round.roundId, recipeId: round.recipeId, state: round.state, simulated: round.simulated, error: round.error, format: 1,
    cameras: round.cameras.map(camera => ({ cameraId: camera.cameraId, frames: camera.receivedCount })),
    groupCount: round.frames.length, incompleteGroups: [...new Set(missing.map(item => item.seq))],
    lossAlarms: [], plcEstimate: round.plcPlannedCount ?? round.plannedCount, deviceTriggers: round.plcActualCount ?? null, missing,
  };
}

/** 本轮 cameraId 这台相机有没有第 seq 帧（seq 从 1 起）。 */
export function frameArrived(view: RoundView, cameraId: string, seq: number): boolean {
  const index = view.cameras.findIndex(camera => camera.cameraId === cameraId);
  if (index < 0 || seq < 1 || seq > view.groupCount) return false;
  return !view.missing.some(item => item.camera === index && item.seq === seq);
}

/** 读取第 seq 帧原图：新轮次按帧号，旧轮次按拍照点下标（seq − 1）。 */
export function readRoundFrame(round: AnyRound, cameraId: string, seq: number): Promise<GrayImage> {
  return isRoundV2(round) ? workspaceApi.captureFrameImage(round.roundId, cameraId, seq) : workspaceApi.captureImage(round.roundId, seq - 1, cameraId);
}

/** 丢帧告警的一句话：某台相机帧数与 PLC 累计触发或另一台相机相差超过 1（只告警，不影响采集和采用）。 */
export function lossAlarmText(alarm: LossAlarm, cameraName: (index: number) => string): string {
  const name = cameraName(alarm.camera);
  return alarm.reason.type === "triggers"
    ? `${name} 收到 ${alarm.count} 帧，与 PLC 累计触发 ${alarm.reason.deviceTriggers} 相差超过 1，疑似丢帧`
    : `${name} 收到 ${alarm.count} 帧，与${cameraName(alarm.reason.other)}（${alarm.reason.otherCount} 帧）相差超过 1，疑似丢帧`;
}
