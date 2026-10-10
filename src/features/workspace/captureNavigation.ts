export interface CaptureReturn {
  recipeId: string;
  search: string;
  cameraId: string;
  planned: number;
  drain: number;
  roundId?: string;
  previewK: number;
}

export interface CaptureNavigationState {
  captureReturn?: CaptureReturn;
}
