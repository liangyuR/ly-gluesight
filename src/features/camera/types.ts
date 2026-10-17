export type CameraSource = "mvs" | "sim" | "sampleSet";

export interface CameraConfig {
  /** 相机编号：配方用它引用相机，建相机时分配、之后不变 */
  id: string;
  name: string;
  source: CameraSource;
  viewCount: number;
  compositeLayout?: {kind:"horizontal" | "vertical"} | {kind:"rects";rects:{x:number;y:number;width:number;height:number}[]} | null;
  serial: string;
  triggerSource: "Line0" | "Software";
  triggerActivation: "RisingEdge" | "FallingEdge";
  triggerDelayUs: number;
  debouncerUs: number;
  strobe: boolean;
  chunk: boolean;
  counterAfterOpen?: number | null;
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
  ready: boolean;
  /** 已释放给海康 MVS 客户端，不自动重连 */
  released: boolean;
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

/** 触发计数来源：chunkTrigger 与 synthetic 才是真的触发计数，其余照填帧计数 */
export type CounterSource = "chunkTrigger" | "chunkFrame" | "sdkFrame" | "synthetic";

export interface Frame {
  cam: number;
  /** 设备会话号：重开相机后换新号，触发计数可能从头数起 */
  session: number;
  counter: CounterSource;
  frameCounter: number;
  triggerCounter: number;
  lostPackets: number;
  ts: number;
}

/** 缩略图：像素为缩略图尺寸，full 为原图尺寸 */
export interface PreviewImage {
  width: number;
  height: number;
  fullWidth: number;
  fullHeight: number;
  data: ImageData;
}
