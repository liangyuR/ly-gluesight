import type { ReturnNavigationState } from "./returnNavigation";

export interface CaptureReturn {
  recipeId: string;
  search: string;
  cameraIds: string[];
  drain: number;
  roundId?: string;
  previewK: number;
}

export interface CaptureNavigationState extends ReturnNavigationState {
  captureReturn?: CaptureReturn;
}
