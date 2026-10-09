import { useState } from "react";
import { usePreviewCanvas } from "../../camera/api";
import type { CameraStatus, FollowCalib, Frame } from "../../camera/types";
import type { Measured } from "../types";
import { CAM_COLORS } from "../vis";

const ST_COLOR = ["var(--ok)", "var(--ng)", "var(--err)"];

interface Props {
  status: CameraStatus;
  calib: FollowCalib | null;
  /** 这台相机最近测的一帧 */
  last: Measured | null;
  active: boolean;
  frame: Frame | undefined;
}

export default function CameraTile({ status, calib, last, active, frame }: Props) {
  const [overlay, setOverlay] = useState(true);
  const { img, canvas } = usePreviewCanvas(status.cam, frame?.frameCounter);
  const color = CAM_COLORS[status.cam % CAM_COLORS.length];

  const [fw, fh] = img ? [img.fullWidth, img.fullHeight] : calib ? calib.imageSize : [4, 3];
  const ok = last ? last.st.filter((s) => s === 0).length : 0;
  const widths = last ? (last.w.filter((v, i) => v != null && last.st[i] === 0) as number[]) : [];
  const meanW = widths.length ? widths.reduce((a, b) => a + b, 0) / widths.length : null;

  return (
    <div className={`cam-tile${active ? " active" : ""}${status.ready ? "" : " down"}`} style={{ ["--cam" as string]: color }}>
      <div className="cam-tile-head" style={{ flexWrap: "wrap" }}>
        <i />
        <b>{status.name}</b>
        <span className="mono">{!status.ready ? "未就绪" : status.fps ? `${status.fps.toFixed(1)} fps` : "待机"}</span>
        {status.droppedFrames > 0 && <span className="c-warn mono">丢 {status.droppedFrames}</span>}
        {active && <span className="tag">测量中</span>}
        {(calib || last?.px.length) && <button type="button" className="btn small" aria-label={`${status.name}测量叠加`} aria-pressed={overlay} onClick={() => setOverlay(value => !value)}>叠加</button>}
      </div>
      <div className="cam-tile-img">
        <canvas ref={canvas} style={{ display: img ? "block" : "none" }} />
        {!img && <span className="muted">{status.ready ? "暂无图像" : status.message}</span>}
        <svg viewBox={`0 0 ${fw} ${fh}`} aria-label={`${status.name}测量图层`}>
          {overlay && calib && (
            <circle cx={calib.nozzle[0]} cy={calib.nozzle[1]} r={Math.max(calib.maskPx, 6)} fill="none" stroke={color} strokeDasharray="6 5" strokeWidth={2} vectorEffect="non-scaling-stroke" />
          )}
          {overlay && last?.px.map(([x, y], i) => <circle key={i} cx={x} cy={y} r={3.5} fill={ST_COLOR[last.st[i]] ?? "var(--err)"} vectorEffect="non-scaling-stroke" />)}
        </svg>
      </div>
      <div className="cam-tile-foot mono">
        {last ? (
          last.error ? (
            <span className="c-err">{last.error}</span>
          ) : (
            <>
              k={last.k}
              {last.s != null && ` · s=${last.s.toFixed(1)}`} · {ok}/{last.idx.length} 点{meanW != null && ` · 胶宽 ${meanW.toFixed(2)}`}
            </>
          )
        ) : (
          <span className="muted">{status.message}</span>
        )}
      </div>
    </div>
  );
}
