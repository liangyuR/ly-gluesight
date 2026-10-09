import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { Save, Zap } from "lucide-react";
import { cameraApi, usePreviewCanvas, useRigStatus } from "../../camera";
import type { Recipe } from "../../cycle/types";

interface TeachStatus {
  dir: string;
  taught: boolean[];
  stale: boolean;
  /** 各拍照点所用工位标定给出的像素当量；没标定为 null */
  mmPerPx: (number | null)[];
}

/**
 * 飞拍示教（真实相机 / 回放）：机器人停在拍照点 k，取一帧；把名义测量点按像素当量叠到图上，
 * 平移旋转对齐到实际内边；框选模板（跨内边与特征孔，避开胶条）后保存。
 */
export default function FlyshotTeach({ recipe }: { recipe: Recipe }) {
  const { statuses, lastFrame } = useRigStatus();
  const [k, setK] = useState(0);
  const shot = recipe.shots[k];
  // 拍照点按编号引用相机，取图、触发按它此刻在相机组里的序号
  const cam = statuses.find((s) => s.id === shot?.camera)?.cam ?? 0;
  const { img, canvas } = usePreviewCanvas(cam, lastFrame[cam]?.frameCounter);
  const svg = useRef<SVGSVGElement>(null);
  const [status, setStatus] = useState<TeachStatus | null>(null);
  const [align, setAlign] = useState({ dx: 0, dy: 0, deg: 0, mmPerPx: 0.04 });
  const [rect, setRect] = useState<[number, number, number, number] | null>(null);
  const [drag, setDrag] = useState<[number, number] | null>(null);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);

  const refresh = () => {
    if (!isTauri()) return;
    invoke<TeachStatus>("teach_flyshot_status", { recipeId: recipe.id })
      .then(setStatus)
      .catch((e) => setNotice({ ok: false, text: String(e) }));
  };
  useEffect(refresh, [recipe.id, recipe.hash]);
  // 每个拍照点用自己的工位标定：换拍照点或标定刷新后取它的像素当量
  const calibMm = status?.mmPerPx[k];
  useEffect(() => {
    if (calibMm) setAlign((a) => ({ ...a, mmPerPx: Number(calibMm.toFixed(5)) }));
  }, [calibMm]);

  const [fw, fh] = img ? [img.fullWidth, img.fullHeight] : [2448, 2048];
  // 名义测量点 → 像素：以图像中心为拍照点中心，按像素当量缩放，再平移旋转
  const overlay = useMemo(() => {
    const [cx, cy] = recipe.shots[k]?.center ?? [0, 0];
    const [s, c] = [Math.sin((align.deg * Math.PI) / 180), Math.cos((align.deg * Math.PI) / 180)];
    const [icx, icy] = [fw / 2 + align.dx, fh / 2 + align.dy];
    const pts: string[] = [];
    recipe.points.k.forEach((owner, j) => {
      if (owner !== k) return;
      const u = (recipe.points.x[j] - cx) / align.mmPerPx;
      const v = (recipe.points.y[j] - cy) / align.mmPerPx;
      pts.push(`${(icx + u * c - v * s).toFixed(1)},${(icy + u * s + v * c).toFixed(1)}`);
    });
    return pts.join(" ");
  }, [recipe, k, align, fw, fh]);

  const toImage = (e: MouseEvent<SVGSVGElement>): [number, number] | null => {
    const m = svg.current?.getScreenCTM();
    if (!m) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m.inverse());
    return [Math.round(p.x), Math.round(p.y)];
  };
  const down = (e: MouseEvent<SVGSVGElement>) => {
    const p = toImage(e);
    if (p) {
      setDrag(p);
      setRect([p[0], p[1], 0, 0]);
    }
  };
  const move = (e: MouseEvent<SVGSVGElement>) => {
    const p = drag && toImage(e);
    if (!drag || !p) return;
    setRect([Math.min(drag[0], p[0]), Math.min(drag[1], p[1]), Math.abs(p[0] - drag[0]), Math.abs(p[1] - drag[1])]);
  };

  const trigger = () => cameraApi.softTrigger(cam).catch((e) => setNotice({ ok: false, text: String(e) }));
  const save = async () => {
    if (!rect) return setNotice({ ok: false, text: "先在图上拖出模板矩形" });
    try {
      const s = await invoke<TeachStatus>("teach_flyshot_save", { teach: { recipeId: recipe.id, k, rect, ...align } });
      setStatus(s);
      setNotice({ ok: true, text: `拍照点 ${shot?.id ?? `k=${k}`} 已示教` });
    } catch (e) {
      setNotice({ ok: false, text: String(e) });
    }
  };

  const num = (label: string, key: keyof typeof align, step: number) => (
    <label className="field">
      <span>{label}</span>
      <input className="input mono" type="number" step={step} value={align[key]} onChange={(e) => setAlign({ ...align, [key]: Number(e.target.value) })} />
    </label>
  );

  return (
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">飞拍示教</h3>
        <span className="muted">真实相机 / 回放用；模拟相机自动示教。{status?.stale && <b className="c-warn"> 胶路或拍照点改过，需要重新示教</b>}</span>
        <span className="spacer" />
        <button className="btn" onClick={trigger}>
          <Zap size={15} />
          取一帧（{shot?.camera ?? "—"}）
        </button>
        <button className="btn primary" onClick={save} disabled={!img}>
          <Save size={15} />
          保存 {shot?.id ?? `k=${k}`}
        </button>
      </div>
      <div className="teach-shots">
        {recipe.shots.map((s, i) => (
          <button key={i} className={`chip${i === k ? " active" : ""}${status?.taught[i] ? " done" : ""}`} title={`k=${i} · Pose ${s.poseId}`} onClick={() => setK(i)}>
            {s.id} · {s.camera} {status?.taught[i] ? "✓" : ""}
          </button>
        ))}
      </div>
      <div className="calib-view">
        <canvas ref={canvas} style={{ display: img ? "block" : "none" }} />
        {!img && <span className="muted">机器人停在拍照点 {shot?.id ?? `k=${k}`}（Pose {shot?.poseId ?? "—"}），点“取一帧”</span>}
        <svg ref={svg} viewBox={`0 0 ${fw} ${fh}`} onMouseDown={down} onMouseMove={move} onMouseUp={() => setDrag(null)}>
          <polyline points={overlay} fill="none" stroke="var(--accent-text)" strokeWidth={2} vectorEffect="non-scaling-stroke" />
          {rect && <rect x={rect[0]} y={rect[1]} width={rect[2]} height={rect[3]} fill="var(--accent-overlay)" stroke="var(--accent-text)" vectorEffect="non-scaling-stroke" />}
        </svg>
      </div>
      <div className="calib-row four">
        {num("平移 x（px）", "dx", 1)}
        {num("平移 y（px）", "dy", 1)}
        {num("旋转（°）", "deg", 0.1)}
        {num("像素当量（mm/px）", "mmPerPx", 0.0005)}
      </div>
      <p className="muted hint">
        黄线是这一帧负责的名义测量点，调平移、旋转让它落在实际内边上；拖出的蓝框是定位模板（跨内边与安装孔，避开胶条）。示教资料存在 {status?.dir ?? "数据目录"}。
      </p>
      {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`}>{notice.text}</div>}
    </div>
  );
}
