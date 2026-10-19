import { CameraStrip, type CameraFrame, type CameraFrameState } from "qing-steel";
import type { FrameView, PartView, PointVis, Recipe } from "../types";
import { shotCameras, shotLabel, shotSegment, shotState } from "../vis";
import { ShotLine } from "./ShotTiles";

function footer(f: FrameView) {
  if (f.error) return f.error;
  switch (f.status) {
    case "waiting":
      return "—";
    case "measuring":
      return `到达 ${f.arrivedMs ?? "—"} ms · 测量中`;
    case "done":
      return `${f.score?.toFixed(2) ?? "—"} · ${f.points} 点 · ${f.ms ?? "—"} ms`;
    case "locateFailed":
      return `定位分数 ${f.score?.toFixed(2) ?? "—"}`;
    case "error":
      return "见事件日志";
    case "missing":
      return f.counterJump ? "帧计数跳号" : "未到达";
  }
}

function frameState(layout: Recipe, vis: PointVis[], k: number, f: FrameView | undefined): CameraFrameState {
  if (layout.shots[k].skip) return "skipped";
  switch (f?.status ?? "waiting") {
    case "waiting": return "waiting";
    case "measuring": return "capturing";
    case "missing": return "missing";
    case "locateFailed":
    case "error": return "fault";
  }
  if (f?.gapPoints) return "ng";
  const state = shotState(layout, vis, k);
  return state === "ng" || state === "gap" ? "ng" : state === "inv" ? "fault" : state === "miss" ? "missing" : "ok";
}

export default function ShotStrip({ layout, part, vis, selected, onSelect, compact = false }: { layout: Recipe; part: PartView | null; vis: PointVis[]; selected?:number; onSelect?:(k:number)=>void; compact?: boolean }) {
  const own = part && part.recipeId === layout.id && part.recipeRevision === layout.revisionId ? part : null;
  const cameras = shotCameras(layout.shots);
  const frames: CameraFrame[] = layout.shots.map((shot, k) => {
    const f = own?.frames[k];
    const state = frameState(layout, vis, k, f);
    const attention = f?.viewResults?.filter(view => view.verdict !== "OK" || !!view.error).length ?? 0;
    const bad = state === "fault" || state === "missing";
    const detail = [f?.gapPoints && `缺胶 ${f.gapPoints}`, f && (bad || !compact) && footer(f), compact && attention > 0 && `检测图注意 ${attention}`].filter(Boolean).join(" · ");
    return {
      id: `k${k + 1}`,
      cameraId: shot.camera,
      label: compact ? shot.id : shotLabel(shot, k),
      cycleId: own?.cycleId,
      state,
      preview: shotSegment(layout, k) ? <ShotLine layout={layout} k={k} vis={vis} compact /> : <span>未示教</span>,
      detail: detail || undefined,
      channel: ((cameras.indexOf(shot.camera) % 8) + 1) as CameraFrame["channel"],
    };
  });
  return <CameraStrip label="拍照点" frames={frames} cycleId={own?.cycleId} value={selected == null ? null : `k${selected + 1}`} onValueChange={id => onSelect?.(Number(id.slice(1)) - 1)} />;
}
