import { useId, useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { Button, Card, StatusBadge } from "qing-steel";
import { displayReason, verdictLabel, verdictTone } from "../../history/meta";
import type { PathSegment } from "../../plan/types";
import { expectedText, flagBadges, groupCellText, offsetUnjudgedText, regionKindLabel, uncheckedText, uninspectedText } from "../../plan/vis";
import { formatTs } from "../../plc/time";
import { buildPartEvidence } from "../partEvidence";
import type { PartView, PlcWriteTrace, ResultView } from "../types";

type Detail = "coverage" | "verdict" | "plc";
type Tone = "ok" | "ng" | "warn" | "fault" | "neutral";
type Locate = (seq: number, camera?: number) => void;

interface Summary {
  label: string;
  value: string;
  tone: Tone;
  detail?: Detail;
  hint?: string;
}

const plcStatus: Record<PlcWriteTrace["state"], Pick<Summary, "value" | "tone">> = {
  written: { value: "写入完成", tone: "ok" },
  failed: { value: "写入未确认", tone: "fault" },
  notWritten: { value: "未回写", tone: "warn" },
};

const detailTitles: Record<Detail, string> = {
  coverage: "检测覆盖明细", verdict: "判定证据", plc: "本件 PLC 通讯",
};

function frameRange(sequences: number[]) {
  if (sequences.length === 1) return `帧组 ${sequences[0]}`;
  return `帧组 ${sequences[0]}–${sequences[sequences.length - 1]} 间共 ${sequences.length} 组`;
}

function EvidenceList({ children }: { children: ReactNode }) {
  return <ul className="prod-evidence-list">{children}</ul>;
}

function PlcTrace({ trace, result }: { trace: PlcWriteTrace; result: ResultView }) {
  const [showEvents, setShowEvents] = useState(false);
  const timelineId = useId();
  const maintained = result.mode === "maintenanceCollect" || result.mode === "maintenanceShadow";
  const events = [
    [trace.receivedAt, "PC 接到工件"],
    [trace.endedAt, "PC 读到工件结束"],
    [trace.updatedAt, plcStatus[trace.state].value],
  ] as const;
  return <>
    <p>{trace.state === "written" ? (maintained ? "维护模式 · PC 已写入并读回结果码 0，PLC 按未检放行。" : "PC 已写入并读回本件结果。") : trace.message || plcStatus[trace.state].value}</p>
    <p className="muted">{trace.state === "written" ? "确认 PLC 是否取走本件结果，请在 PLC 侧核对件序号。" : "请检查 PLC 通讯和本件序号，核对 PLC 侧实际收到的结果。"}</p>
    <Button size="sm" variant="tonal" aria-expanded={showEvents} aria-controls={timelineId} onClick={() => setShowEvents(!showEvents)}>
      {showEvents ? "收起写入记录" : "查看写入记录"}
    </Button>
    <ol id={timelineId} hidden={!showEvents} className="prod-plc-events">
      {events.map(([ts, description]) => ts != null && <li key={description}><time className="mono" dateTime={new Date(ts).toISOString()}>{formatTs(ts).split(" ")[1]}</time><span>{description}</span></li>)}
    </ol>
  </>;
}

export default function PartResult({ phase, result, part, segments, onLocate }: {
  phase: string;
  result: ResultView | null;
  part: PartView | null;
  segments: readonly PathSegment[];
  onLocate: Locate;
}) {
  const [selected, setSelected] = useState<Detail | null>(null);
  const id = useId();
  const activeButton = useRef<HTMLButtonElement | null>(null);
  const burst = part?.burst ?? null;
  const evidence = useMemo(() => result && buildPartEvidence(result, burst, segments), [result, burst, segments]);
  const running = !result && phase !== "IDLE" && phase !== "FAULT";
  const collecting = result?.mode === "maintenanceCollect";
  const trial = result?.mode === "maintenanceShadow";
  const [code, description] = result ? verdictLabel[result.verdict].split(" · ") : [];
  const reason = result && !collecting ? displayReason(result.reason) : null;
  const tone = result && !collecting ? verdictTone(result.verdict) : "neutral";
  const trace = result?.plcWrite;
  const coverage = evidence?.coverage;
  const incomplete = coverage?.status === "incomplete";
  const offsetUnjudged = coverage?.status === "offsetUnjudged";
  const flags = flagBadges(result?.burst?.flags).filter(flag => flag.key !== "incomplete" && flag.key !== "offsetUnjudged");
  const issue = result && !collecting && (tone === "ng" || tone === "fault");
  const firstDefect = evidence?.defects[0];
  const firstArea = coverage?.affected[0];
  const firstCamera = firstArea?.cameras[0];
  const coverageHint = firstArea && [regionKindLabel(firstArea.region.kind), firstCamera && `${firstCamera.name} ${groupCellText[firstCamera.status]} ${firstCamera.sequences.length} 帧`].filter(Boolean).join(" · ");
  const summaries: Summary[] = result ? [
    ...(issue ? [{ label: "判定证据", value: description ?? code, tone, detail: "verdict" as const,
      hint: firstDefect ? regionKindLabel(firstDefect.region.kind) : undefined }] : []),
    incomplete || offsetUnjudged ? { label: "检测完整性", value: incomplete ? "检测不完整" : "胶偏未判", tone: "warn", detail: "coverage", hint: coverageHint }
      : { label: "检测完整性", value: coverage?.status === "complete" ? "计划内完整" : "未判定", tone: coverage?.status === "complete" ? "ok" : "neutral" },
    trace ? { label: "PC → PLC", ...plcStatus[trace.state], detail: "plc" }
      : { label: "PC → PLC", value: "无写入记录", tone: "neutral" },
  ] : [];
  const detail = summaries.find(summary => summary.detail === selected);

  function locate(label: string, seq: number | null, camera?: number) {
    if (seq == null) return null;
    return <Button size="sm" variant="tonal" onClick={() => onLocate(seq, camera)}>{label}</Button>;
  }

  return <>
    <Card className="prod-part-result" role="region" aria-label="整件判定">
      <div className="prod-part-heading">
        <h3 className="panel-title">整件结果</h3>
        {trace && <span className="muted mono">件序号 #{trace.partSeq}</span>}
      </div>
      <div className="prod-part-verdict" data-tone={tone} aria-live="polite">
        <strong className="prod-part-code mono">{collecting ? "—" : code ?? (running ? <span className="prod-part-pulse" /> : "—")}</strong>
        <div className="prod-part-caption">
          {result && <span className="muted">{trial ? "维护试判" : "产品判定"}</span>}
          <strong>{result ? collecting ? "维护采集 · 未判定" : description ?? "整件合格" : running ? "检测中…" : "暂无检测结果"}</strong>
          {!result && running && burst && <span className="muted mono">帧组 {burst.groupCount} / {expectedText(burst.expectedTotal)}</span>}
          {!result && !running && phase !== "FAULT" && <span className="muted">等待工件</span>}
        </div>
      </div>
      {result && <div className="prod-part-context">
        <span className="mono" title={`SN ${result.sn}`}>SN {result.sn}</span>
        {trace?.state === "written" && <span className="mono">结果码 {trial || collecting ? 0 : result.plcCode}{!trial && !collecting && result.faultCode ? ` / ${result.faultCode}` : ""}</span>}
      </div>}
      {!issue && reason && <p className="prod-part-reason muted">{reason}</p>}
      {flags.length > 0 && <div className="prod-part-flags">{flags.map(flag => <StatusBadge key={flag.key} tone={flag.tone}>{flag.label}</StatusBadge>)}</div>}
      {summaries.map(summary => {
        const panel = summary.detail;
        const expanded = !!panel && panel === selected;
        const content = <>
          <span>{summary.label}</span>
          <strong data-tone={summary.tone}>{summary.value}{panel && (expanded ? <ChevronDown size={14} aria-hidden="true" /> : <ChevronRight size={14} aria-hidden="true" />)}</strong>
          {summary.hint && <small className="muted">{summary.hint}</small>}
        </>;
        return panel
          ? <button key={summary.label} type="button" className="prod-part-summary" data-tone={summary.tone} aria-expanded={expanded} aria-controls={expanded ? id : undefined} onClick={event => {
            activeButton.current = event.currentTarget;
            setSelected(expanded ? null : panel);
          }}>{content}</button>
          : <div key={summary.label} className="prod-part-summary">{content}</div>;
      })}
    </Card>
    {selected && detail && <Card className="prod-part-evidence" data-tone={detail.tone} id={id} role="region" aria-label={detailTitles[selected]}>
      <div className="prod-part-heading">
        <h3 className="panel-title">{detailTitles[selected]}</h3>
        <Button size="sm" variant="text" onClick={() => { setSelected(null); activeButton.current?.focus(); }}>收起</Button>
      </div>
      <div className="prod-evidence-body">
        {selected === "coverage" && coverage && <>
          {coverage.affected.length > 0 ? <EvidenceList>{coverage.affected.map(area => <li key={area.region.firstSeq}>
            <strong>{regionKindLabel(area.region.kind)}</strong>
            {area.sequences.length > 0 && <p>{frameRange(area.sequences)} · 检测不完整</p>}
            {area.cameras.length > 0 && <ul className="prod-evidence-list">{area.cameras.map(camera => <li key={`${camera.camera}:${camera.status}`}>
              <p>{camera.name} · {groupCellText[camera.status]} · {frameRange(camera.sequences)}</p>
              {locate(`定位到帧组 ${camera.sequences[0]}`, camera.sequences[0], camera.camera)}
            </li>)}</ul>}
            {area.cameras.length === 0 && locate("定位到不完整帧组", area.locateSeq)}
          </li>)}</EvidenceList> : incomplete && <p>本件有检测不完整标记，当前没有可定位的帧组证据。</p>}
          {coverage.offsetUnjudged.length > 0 && <EvidenceList>{coverage.offsetUnjudged.map(item => <li key={item.region.firstSeq}>
            <strong>{regionKindLabel(item.region.kind)}</strong>
            <p>{offsetUnjudgedText[item.reason]}{item.camera && ` · ${item.camera}`}</p>
          </li>)}</EvidenceList>}
          {offsetUnjudged && coverage.offsetUnjudged.length === 0 && <p>本件胶偏未判，当前没有可定位的区域证据。</p>}
          {coverage.uninspected.length > 0 && <p className="muted">计划跳过：{coverage.uninspected.map(uninspectedText).join("、")}</p>}
          {coverage.unchecked.length > 0 && <p className="muted">本轮未检项：{coverage.unchecked.map(item => uncheckedText[item]).join("、")}</p>}
          {incomplete && <p className="muted">若后续工件重复出现，缺帧请检查相机和 Line0 触发，未测成请复核原图及采集详情。</p>}
        </>}
        {selected === "verdict" && <>
          {evidence && evidence.defects.length > 0 ? <EvidenceList>{evidence.defects.map(item => <li key={item.region.firstSeq}>
            <strong>{regionKindLabel(item.region.kind)} · {verdictLabel[item.verdict]}</strong>
            {item.frame && <p>{item.frame.name} · 帧组 {item.frame.seq}</p>}
            {item.reason && <p>{displayReason(item.reason)}</p>}
            {locate(item.frame ? "定位到异常帧" : "查看该区域原图", item.locateSeq, item.frame?.camera)}
          </li>)}</EvidenceList> : <p>{reason || "当前没有可定位的区域证据。"}</p>}
          <p className="muted">{tone === "ng" ? "对照本件原图复核受影响的胶路。" : "核对本件原因及上方就绪状态。"}</p>
        </>}
        {selected === "plc" && trace && result && <PlcTrace trace={trace} result={result} />}
      </div>
    </Card>}
  </>;
}
