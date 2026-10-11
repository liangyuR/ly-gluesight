export interface CaptureReturn {
  recipeId: string;
  search: string;
  cameraId: string;
  drain: number;
  roundId?: string;
  previewK: number;
}

export interface CaptureNavigationState {
  captureReturn?: CaptureReturn;
}
