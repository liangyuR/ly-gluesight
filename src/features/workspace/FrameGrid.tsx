import { useEffect, useRef, type KeyboardEvent } from "react";
import "../cycle/components/burst.css";
import "./capture.css";

/**
 * 相机 × 帧组点选格，外观同历史详情的帧组条：每个帧组一列、每台相机一格，点格子选相机与帧组，点列号只换帧组。
 * 方向键左右换帧组、上下换相机。缺帧的格子可选但标虚线；给了 range 时范围外的列不能选。camera 为 cameras 下标，seq 从 1 起。
 */
export function FrameGrid({ label, cameras, groupCount, camera, seq, onSelect, missing, range }: {
  label: string;
  cameras: { id: string; name: string }[];
  groupCount: number;
  camera: number;
  seq: number;
  onSelect: (camera: number, seq: number) => void;
  missing?: (camera: number, seq: number) => boolean;
  range?: { start: number; end: number } | null;
}) {
  const root = useRef<HTMLDivElement>(null);
  const first = range?.start ?? 1, last = range?.end ?? groupCount;
  useEffect(() => {
    const cell = root.current?.querySelector<HTMLElement>(".fg-cell-button[aria-pressed='true']");
    cell?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    if (root.current?.contains(document.activeElement) && document.activeElement !== cell) cell?.focus();
  }, [camera, seq]);
  const key = (e: KeyboardEvent) => {
    const step = { ArrowLeft: [0, -1], ArrowRight: [0, 1], ArrowUp: [-1, 0], ArrowDown: [1, 0] }[e.key];
    if (!step) return;
    e.preventDefault();
    onSelect(Math.min(Math.max(0, camera + step[0]), cameras.length - 1), Math.min(Math.max(first, seq + step[1]), last));
  };
  const groups = Array.from({ length: groupCount }, (_, i) => i + 1);
  return <div ref={root} className="fg-strip wp-frame-grid" role="group" aria-label={label} onKeyDown={key}>
    <div className="fg-cams" aria-hidden="true">{cameras.map(item => <span key={item.id} title={item.name}>{item.name}</span>)}</div>
    <ol className="fg-groups">
      {groups.map(s => {
        const outside = s < first || s > last;
        const gaps = cameras.map((_, c) => !!missing?.(c, s));
        return <li key={s}>
          <div className={"fg-group fg-individual" + (s === seq ? " selected" : "") + (gaps.some(Boolean) ? " incomplete" : "") + (outside ? " outside" : "")}>
            <button type="button" className="fg-seq fg-seq-button" aria-label={`帧组 ${s}`} disabled={outside} onClick={() => onSelect(camera, s)}>{s}</button>
            {cameras.map((item, c) => <button key={item.id} type="button" className="fg-cell fg-cell-button" data-state={gaps[c] ? "missing" : "available"} disabled={outside}
              aria-label={`帧组 ${s} · ${item.name}${gaps[c] ? " · 缺帧" : ""}`} title={`${item.name} · 帧组 ${s}${gaps[c] ? " · 缺帧" : outside ? " · 不在本段" : ""}`}
              aria-pressed={s === seq && c === camera} onClick={() => onSelect(c, s)} />)}
          </div>
        </li>;
      })}
    </ol>
  </div>;
}
