import { useEffect, useState } from "react";
import { Button, Checkbox, Table, type TableColumn } from "qing-steel";
import { errorText } from "../../lib/errors";
import ContextHelp from "../cycle/components/ContextHelp";
import type { PartMode } from "../history/types";
import { workspaceApi } from "./api";
import { Badge, Notice, Panel } from "./components";
import type { HistoryCandidate } from "./types";

const modeLabels: Record<PartMode, string> = { production: "生产", maintenanceCollect: "维护采集", maintenanceShadow: "维护试判" };

function timing(r: HistoryCandidate) {
  if (r.durationMs == null && r.measuredFps == null) return "—";
  const text = [r.durationMs != null && `${(r.durationMs / 1000).toFixed(1)} s`, r.measuredFps != null && `${r.measuredFps.toFixed(1)} fps`].filter(Boolean).join(" · ") + (r.setFps != null ? `（设定 ${r.setFps}）` : "");
  const off = r.measuredFps != null && r.setFps != null && r.setFps > 0 && Math.abs(r.measuredFps - r.setFps) > 0.1 * r.setFps;
  return off ? <Badge tone="warn">{text}</Badge> : text;
}

interface Props {
  recipeId: string;
  revision: number;
  disabled: boolean;
  nameOf: (cameraId: string) => string;
  onImported: (roundIds: string[]) => void;
  onError: (message: string) => void;
}

/** 采集轮次页“从历史选用”：维护模式或生产录制的完整件导入为采集轮次（原图硬链接，不停线），可在导入时标记标准件。 */
export default function HistoryImportPanel({ recipeId, revision, disabled, nameOf, onImported, onError }: Props) {
  const [rows, setRows] = useState<HistoryCandidate[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [reload, setReload] = useState(0);
  /** 已勾选的记录 → 是否标准件 */
  const [picked, setPicked] = useState<Record<number, boolean>>({});
  const [working, setWorking] = useState(false);
  useEffect(() => {
    let alive = true;
    setLoadError("");
    workspaceApi.historyCandidates(recipeId).then(r => { if (alive) setRows(r); }).catch(e => { if (alive) { setRows([]); setLoadError(errorText(e)); } });
    return () => { alive = false; };
  }, [recipeId, reload]);
  useEffect(() => { setPicked({}); }, [recipeId]);
  const usable = rows?.filter(r => !r.reasons.length).length ?? 0;
  const chosen = Object.keys(picked).length;
  const pick = (id: number, on: boolean) => setPicked(p => { const next = { ...p }; if (on) next[id] = false; else delete next[id]; return next; });
  const importPicked = async () => {
    if (!chosen || working) return;
    setWorking(true);
    try {
      const rounds = await workspaceApi.historyImport(recipeId, revision, Object.entries(picked).map(([id, standard]) => ({ recordId: Number(id), standard })));
      setPicked({});
      onImported(rounds);
    } catch (e) { onError(errorText(e)); } finally { setWorking(false); setReload(n => n + 1); }
  };
  const columns: TableColumn<HistoryCandidate>[] = [
    { key: "use", header: "选用", render: r => <Checkbox label={<span className="qs-sr-only">{"选用 SN " + r.sn}</span>} checked={r.recordId in picked} disabled={disabled || working || r.reasons.length > 0} onCheckedChange={on => pick(r.recordId, on)}/> },
    { key: "time", header: "时间", render: r => new Date(r.ts).toLocaleString("zh-CN") },
    { key: "sn", header: "SN", render: r => <span className="mono">{r.sn}</span> },
    { key: "mode", header: "模式", render: r => modeLabels[r.mode] },
    { key: "groups", header: "帧组", render: r => r.groupCount },
    { key: "frames", header: "各相机帧数", render: r => r.cameras.map(c => `${nameOf(c.cameraId)} ${c.frames}`).join(" · ") || "—" },
    { key: "timing", header: "时长 / 实测帧率（设定）", render: timing },
    { key: "complete", header: "原图", render: r => <Badge tone={r.complete ? "ok" : "warn"}>{r.complete ? "完整" : "不完整"}</Badge> },
    { key: "reason", header: "不可用原因", render: r => <>{r.reasons.length ? r.reasons.join("；") : <span className="muted">可导入</span>}{r.warnings.map(w => <span key={w}> <Badge tone="warn">{w}</Badge></span>)}</> },
    { key: "standard", header: "标准件", render: r => <Checkbox label={<span className="qs-sr-only">{"SN " + r.sn + " 是标准件"}</span>} checked={picked[r.recordId] === true} disabled={disabled || working || !(r.recordId in picked)} onCheckedChange={on => setPicked(p => ({ ...p, [r.recordId]: on }))}/> },
  ];
  return <Panel title="从历史选用" titleHelp={<ContextHelp label="从历史选用说明">
    <p>维护模式或生产录制的完整件导入为采集轮次，之后走廊示教、分段复核与多轮标定都按普通轮次处理。</p>
    <p>可导入：原图完整（无缺帧、无录制队满未录）、三台计划相机齐全且设备配置与当前一致、没有提前结束、型号属于本候选。多轮标定至少需要 10 轮，另留几件不导入，作独立验证。</p>
    <p>生产件默认不当标准件：只有按 SN 等核对确认是标准件时才勾选“标准件”；不勾选时 x_ref 留空、胶偏未判，不阻断发布。</p>
  </ContextHelp>} actions={rows && <Badge tone={usable ? "ok" : "warn"}>{`可导入 ${usable} / ${rows.length} 件`}</Badge>}>
    {loadError && <Notice title="读取历史件失败" tone="warn">{loadError}</Notice>}
    <Table caption="可导入的历史件" columns={columns} rows={rows ?? []} rowKey={r => String(r.recordId)} emptyState={<p className="muted center">{rows ? "本配方还没有录制件。开启维护模式跟着生产采图后，可在这里选用。" : "正在读取历史件…"}</p>}/>
    <div className="wp-actions"><Button variant="filled" disabled={disabled || working || !chosen} onClick={() => void importPicked()}>{chosen ? `导入选中的 ${chosen} 件` : "导入选中的历史件"}</Button></div>
  </Panel>;
}
