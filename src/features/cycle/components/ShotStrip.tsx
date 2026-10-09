import type { FrameView, PartView, PointVis, Recipe } from "../types";
import { shotLabel, shotSegment } from "../vis";
import { ShotLine } from "./ShotTiles";

const labels: Record<FrameView["status"], [string, string]> = {
  waiting: ["等待", "c-mut"],
  measuring: ["测量", "c-acc"],
  done: ["完成", "c-ok"],
  locateFailed: ["定位失败", "c-err"],
  error: ["测量出错", "c-err"],
  missing: ["未收到", "c-err"],
};

function footer(f: FrameView) {
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

export default function ShotStrip({ layout, part, vis, selected, onSelect }: { layout: Recipe; part: PartView | null; vis: PointVis[]; selected?:number; onSelect?:(k:number)=>void }) {
  const frames = part && part.recipeId === layout.id && part.recipeHash === layout.hash ? part.frames : null;
  return (
    <div className="shot-strip" style={{ gridTemplateColumns: `repeat(${layout.shots.length}, minmax(0, 1fr))` }}>
      {layout.shots.map((shot, k) => {
        const f = frames?.[k];
        const [label, tone] = labels[f?.status ?? "waiting"];
        const bad = f && (f.status === "locateFailed" || f.status === "error" || f.status === "missing");
        return (
          <button key={k} type="button" className={`shot s-${f?.status ?? "waiting"}${f?.gapPoints ? " has-gap" : ""}${selected===k ? " selected" : ""}`} aria-label={`查看帧 k${k+1}`} aria-pressed={selected===k} onClick={()=>onSelect?.(k)}>
            <div className="shot-head">
              <b className="mono" title={`k${k + 1} · Pose ${shot.poseId}`}>{shotLabel(shot, k)}</b>
              <span className={f?.gapPoints ? "c-ng" : tone}>{f?.gapPoints ? `缺胶 ${f.gapPoints}` : label}</span>
            </div>
            {shot.skip || !shotSegment(layout, k) ? (
              <div className="shot-img shot-img-empty">{shot.skip ? "不检" : "未示教"}</div>
            ) : (
              <ShotLine layout={layout} k={k} vis={vis} compact className="shot-img" />
            )}
            <div className={`shot-foot mono${bad ? " c-err" : ""}`}>{f ? footer(f) : "—"}</div>
          </button>
        );
      })}
    </div>
  );
}
