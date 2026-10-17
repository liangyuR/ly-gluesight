import type { ReturnNavigationState } from "./returnNavigation";

export interface CaptureReturn {
  recipeId: string;
  search: string;
  cameraIds: string[];
  /** 操作员手动取消勾选的相机；其余已配置相机（含之后新增的）默认参与采集 */
  excludedCameraIds: string[];
  drain: number;
  roundId?: string;
  previewK: number;
}

export interface CaptureNavigationState extends ReturnNavigationState {
  captureReturn?: CaptureReturn;
}
