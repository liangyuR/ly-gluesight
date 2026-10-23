import { useEffect, useRef } from "react";
import type { FrameGroupView } from "../../plan/types";
import { groupCellColor, groupCellState, groupCellText } from "../../plan/vis";
import "./burst.css";

/**
 * 帧组条（段模型）：每个 seq 一列，列里每台计划相机一格，同一触发序号的三台相机各一帧。
 * 缺帧、未测、走出走廊的格子是灰色（只算“检测不完整”，不是 NG）；整组有缺格的列号标黄。
 * 格子颜色是该帧单独的结论（在线）或所在区域的结论（历史，没有逐帧结论）；颜色只是显示，结论以后端为准。
 */
export default function FrameGroupStrip({ groups, cameras, selected, onSelect, label = "帧组" }: {
  groups: FrameGroupView[];
  /** 计划相机，按 cells 的顺序 */
  cameras: string[];
  /** 选中的 seq；null 为不选 */
  selected?: number | null;
  onSelect?: (seq: number) => void;
  label?: string;
}) {
  const current = useRef<HTMLButtonElement>(null);
  useEffect(() => { current.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" }); }, [selected]);
  return <div className="fg-strip" role="group" aria-label={label}>
    <div className="fg-cams" aria-hidden="true">{cameras.map(camera => <span key={camera} title={camera}>{camera}</span>)}</div>
    <ol className="fg-groups">
      {groups.map(group => <li key={group.seq}>
        <button type="button" ref={group.seq === selected ? current : undefined}
          className={"fg-group" + (group.seq === selected ? " selected" : "") + (group.incomplete ? " incomplete" : "")}
          aria-pressed={group.seq === selected} aria-label={`帧组 ${group.seq}${group.incomplete ? " · 不完整" : ""}`}
          onClick={() => onSelect?.(group.seq)}>
          <span className="fg-seq">{group.seq}</span>
          {cameras.map((camera, c) => {
            const state = groupCellState(group, c);
            return <i key={camera} role="img" className="fg-cell" data-state={state} aria-label={`${camera} · ${groupCellText[state]}`} title={`${camera} · ${groupCellText[state]}`} style={{ background: groupCellColor[state] }} />;
          })}
        </button>
      </li>)}
    </ol>
  </div>;
}
