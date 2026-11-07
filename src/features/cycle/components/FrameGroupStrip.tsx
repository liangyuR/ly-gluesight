import { useEffect, useRef } from "react";
import type { FrameGroupView, RegionResult } from "../../plan/types";
import { groupCellColor, groupCellState, groupCellText, regionKindLabel, verdictPlanTone, type PlanTone } from "../../plan/vis";
import { verdictLabel } from "../../history/meta";
import "./burst.css";

const bandColor: Record<PlanTone, string> = { ok: "var(--ok)", warn: "var(--warn)", ng: "var(--ng)", info: "var(--primary)", neutral: "var(--outline-variant)" };

/** 段条：帧组上方按区域着色的色带，颜色取区域结论；跳过的区域不判定，为浅灰。 */
function RegionBand({ regions, seq }: { regions: RegionResult[]; seq: number }) {
  const index = regions.findIndex(r => r.region.firstSeq <= seq && seq <= r.region.lastSeq);
  const r = regions[index];
  if (!r) return <span className="fg-band" aria-hidden="true" />;
  const skipped = r.region.kind.type === "skipped";
  const text = `${regionKindLabel(r.region.kind)} · ${skipped ? "不检测" : verdictLabel[r.verdict]}${r.reason ? "：" + r.reason : ""}`;
  return <span className="fg-band" data-start={seq === r.region.firstSeq || undefined} data-end={seq === r.region.lastSeq || undefined}
    title={text} aria-label={seq === r.region.firstSeq ? text : undefined} role={seq === r.region.firstSeq ? "img" : undefined}
    style={{ background: skipped ? "var(--outline-variant)" : bandColor[verdictPlanTone(r.verdict)] }} />;
}

/**
 * 帧组条（段模型）：每个 seq 一列，列里每台计划相机一格，同一触发序号的三台相机各一帧。
 * 缺帧、未测、走出走廊的格子是灰色（只算“检测不完整”，不是 NG）；整组有缺格的列号标黄。
 * 格子颜色只表示该帧单独的结论（判定后按所在区域的检查项判，在线与历史同一口径）；给了区域时上方画段条，
 * 胶宽不均、整段胶偏、帧数异常等整段性问题只体现在段条上。颜色只是显示，结论以后端为准。
 */
export default function FrameGroupStrip({ groups, cameras, regions, selected, onSelect, selectedCamera, onSelectCell, label = "帧组" }: {
  groups: FrameGroupView[];
  /** 计划相机，按 cells 的顺序 */
  cameras: string[];
  /** 整件的区域判定；给了才画段条（在线判定前没有） */
  regions?: RegionResult[] | null;
  /** 选中的 seq；null 为不选 */
  selected?: number | null;
  onSelect?: (seq: number) => void;
  selectedCamera?: number;
  onSelectCell?: (seq: number, camera: number) => void;
  label?: string;
}) {
  const current = useRef<HTMLLIElement>(null);
  useEffect(() => { current.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" }); }, [selected]);
  const banded = !!regions?.length;
  return <div className={"fg-strip" + (banded ? " banded" : "")} role="group" aria-label={label}>
    <div className="fg-cams" aria-hidden="true">{cameras.map(camera => <span key={camera} title={camera}>{camera}</span>)}</div>
    <ol className="fg-groups">
      {groups.map(group => <li key={group.seq} ref={group.seq === selected ? current : undefined}>
        {banded && <RegionBand regions={regions!} seq={group.seq} />}
        {onSelectCell ? <div className={"fg-group fg-individual" + (group.seq === selected ? " selected" : "") + (group.incomplete ? " incomplete" : "")}>
          <button type="button" className="fg-seq fg-seq-button" aria-label={`选择帧组 ${group.seq}`} onClick={() => onSelect?.(group.seq)}>{group.seq}</button>
          {cameras.map((camera, c) => {
            const state = groupCellState(group, c);
            return <button key={camera} type="button" className="fg-cell fg-cell-button" data-state={state} aria-label={`帧组 ${group.seq} · ${camera} · ${groupCellText[state]}`} aria-pressed={group.seq === selected && c === selectedCamera} title={`${camera} · ${groupCellText[state]}`} style={{ background: groupCellColor[state] }} onClick={() => onSelectCell(group.seq, c)} />;
          })}
        </div> : <button type="button"
          className={"fg-group" + (group.seq === selected ? " selected" : "") + (group.incomplete ? " incomplete" : "")}
          aria-pressed={group.seq === selected} aria-label={`帧组 ${group.seq}${group.incomplete ? " · 不完整" : ""}`}
          onClick={() => onSelect?.(group.seq)}>
          <span className="fg-seq">{group.seq}</span>
          {cameras.map((camera, c) => {
            const state = groupCellState(group, c);
            return <i key={camera} role="img" className="fg-cell" data-state={state} aria-label={`${camera} · ${groupCellText[state]}`} title={`${camera} · ${groupCellText[state]}`} style={{ background: groupCellColor[state] }} />;
          })}
        </button>}
      </li>)}
    </ol>
  </div>;
}
