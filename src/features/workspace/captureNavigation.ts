import type { ReturnNavigationState } from "./returnNavigation";

export interface CaptureReturn {
  recipeId: string;
  cameraIds: string[];
  roundId?: string;
  previewK: number;
}

export interface CaptureNavigationState extends ReturnNavigationState {
  captureReturn?: CaptureReturn;
}
