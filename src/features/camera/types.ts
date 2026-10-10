export type CameraSource = "sim" | "mvs" | "replay";
export type Acquisition = "triggered" | "freeRun";

export interface CameraConfig {
  /** 相机编号：配方用它引用相机，建相机时分配、之后不变 */
  id: string;
  name: string;
  source: CameraSource;
  viewCount: number;
  serial: string;
  acquisition: Acquisition;
  fps: number;
  triggerSource: "Line0" | "Software";
  triggerActivation: "RisingEdge" | "FallingEdge";
  triggerDelayUs: number;
  debouncerUs: number;
  exposureUs: number;
  gainDb: number;
  strobe: boolean;
  chunk: boolean;
  replayDir: string;
  replayChannel: number;
}

export interface DeviceSummary {
  serial: string;
  model: string;
  userName: string;
  transport: "GigE" | "USB3";
  ip: string | null;
}

export interface CameraStatus {
  cam: number;
  id: string;
  name: string;
  source: CameraSource;
  acquisition: Acquisition;
  ready: boolean;
  message: string;
  device: DeviceSummary | null;
  sdkVersion: string | null;
  frames: number;
  fps: number;
  maxFps: number | null;
  lostPackets: number;
  droppedFrames: number;
  warnings: string[];
}

export interface Frame {
  cam: number;
  frameCounter: number;
  triggerCounter: number;
  lostPackets: number;
  ts: number;
}

export interface DryFrame {
  cam: number;
  tMs: number;
  frameCounter: number;
  triggerCounter: number;
  lostPackets: number;
}

/** 缩略图：像素为缩略图尺寸，full 为原图尺寸 */
export interface PreviewImage {
  width: number;
  height: number;
  fullWidth: number;
  fullHeight: number;
  data: ImageData;
}

export interface RecordEntry {
  path: string;
  name: string;
  frames: number;
}
