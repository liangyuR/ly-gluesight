export type MapPoint = [number, number];

export interface NavigationPresentation {
  rotation: number;
  flipX: boolean;
  flipY: boolean;
  background: "slate" | "light" | "checker";
}

export const defaultPresentation: NavigationPresentation = { rotation: 0, flipX: false, flipY: false, background: "slate" };

export interface NavigationMapData {
  view?: NavigationPresentation;
  format: 1;
  image: { id: string; width: number; height: number; mime: "image/png" };
  points: MapPoint[];
  closed: boolean;
  captureStart: number;
  captureEnd: number;
  cuts: number[];
  confirmed: boolean;
  structureRevision: number;
}

export interface NavigationSegment { index: number; kind: "straight" | "corner" }
export type FrameRange = [number, number];
