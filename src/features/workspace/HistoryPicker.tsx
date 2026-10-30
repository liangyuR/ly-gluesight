import { useEffect, useState, type ReactNode } from "react";
import { Button, Checkbox, Table, type TableColumn } from "qing-steel";
import { errorText } from "../../lib/errors";
import type { PartMode } from "../history/types";
import { workspaceApi } from "./api";
import { burstImageApi } from "./burstImages";
import { Badge, Notice, Panel } from "./components";
import { FrameBrowser } from "./FrameBrowser";
import { pieceTime } from "./pieces";
import type { HistoryCandidate, HistoryPick } from "./types";

const modeLabels: Record<PartMode, string> = { production: "生产", maintenanceCollect: "维护采集", maintenanceShadow: "维护试判" };
const usageLabels = { teaching: "已使用（示教件）", calibration: "已使用（标定件）" } as const;

function timing(r: HistoryCandidate) {
  if (r.durationMs == null && r.measuredFps == null) return "—";
  const text = [r.durationMs != null && `${(r.durationMs / 1000).toFixed(1)} s`, r.measuredFps != null && `${r.measuredFps.toFixed(1)} fps`].filter(Boolean).join(" · ") + (r.setFps != null ? `（设定 ${r.setFps}）` : "");
  const off = r.measuredFps != null && r.setFps != null && r.setFps > 0 && Math.abs(r.measuredFps - r.setFps) > 0.1 * r.setFps;
  return off ? <Badge tone="warn">{text}</Badge> : text;
}

/** 不能选的原因：不可用原因在前，已被本候选使用的标“已使用” */
const blockers = (r: HistoryCandidate) => [...r.reasons, ...(r.usage ? [usageLabels[r.usage]] : [])];

interface Props {
  recipeId: string;
  /** single：示教件页选一件；multi：标定页选多件，可标标准件 */
  mode: "single" | "multi";
  title: string;
  help?: ReactNode;
  disabled: boolean;
  confirmLabel: (count: number) => string;
  nameOf: (cameraId: string) => string;
  /** 提交所选；返回 true 表示已完成，清空勾选并重读历史件 */
  onConfirm: (picks: HistoryPick[]) => Promise<boolean>;
  onError: (message: string) => void;
}

/**
 * 历史件选择器（RB-6）：本配方最近的真机录制件，不可用或已被本候选使用的不能选；
 * 勾选或点“看图”的那一件在旁边按相机、帧组预览录制原图（只读，不写候选）。
 */
export default function HistoryPicker({ recipeId, mode, title, help, disabled, confirmLabel, nameOf, onConfirm, onError }: Props) {
  const [rows, setRows] = useState<HistoryCandidate[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [reload, setReload] = useState(0);
  /** 已勾选的记录 → 是否标准件 */
  const [picked, setPicked] = useState<Record<number, boolean>>({});
  const [previewId, setPreviewId] = useState<number | null>(null);
  const [working, setWorking] = useState(false);
  useEffect(() => {
    let alive = true;
    setLoadError("");
    workspaceApi.historyCandidates(recipeId).then(r => { if (alive) setRows(r); }).catch(e => { if (alive) { setRows([]); setLoadError(errorText(e)); } });
    return () => { alive = false; };
  }, [recipeId, reload]);
  useEffect(() => { setPicked({}); setPreviewId(null); }, [recipeId]);
  const usable = rows?.filter(r => !blockers(r).length).length ?? 0;
  const chosen = Object.keys(picked).length;
  const pick = (id: number, on: boolean) => {
    setPicked(p => {
      if (mode === "single") return on ? { [id]: false } : {};
      const next = { ...p };
      if (on) next[id] = false; else delete next[id];
      return next;
    });
    if (on) setPreviewId(id);
  };
  const confirm = async () => {
    if (!chosen || working) return;
    setWorking(true);
    try {
      if (await onConfirm(Object.entries(picked).map(([id, standard]) => ({ recordId: Number(id), standard })))) { setPicked({}); setReload(n => n + 1); }
    } catch (e) { onError(errorText(e)); } finally { setWorking(false); }
  };
  const columns: TableColumn<HistoryCandidate>[] = [
    { key: "use", header: "选用", render: r => <span title={blockers(r).join("；") || undefined} onFocus={() => setPreviewId(r.recordId)}><Checkbox label={<span className="qs-sr-only">{"选用 SN " + r.sn}</span>} checked={r.recordId in picked} disabled={disabled || working || blockers(r).length > 0} onCheckedChange={on => pick(r.recordId, on)}/></span> },
    { key: "time", header: "时间", render: r => pieceTime(r.ts) },
    { key: "sn", header: "SN", render: r => <span className="mono">{r.sn}</span> },
    { key: "mode", header: "模式", render: r => <>{modeLabels[r.mode]}{r.fixture != null && <> <Badge tone="neutral">手动配置</Badge></>}</> },
    { key: "groups", header: "帧组", render: r => r.groupCount },
    { key: "frames", header: "各相机帧数", render: r => r.cameras.map(c => `${nameOf(c.cameraId)} ${c.frames}`).join(" · ") || "—" },
    { key: "timing", header: "时长 / 实测帧率（设定）", render: timing },
    { key: "complete", header: "原图", render: r => <Badge tone={r.complete ? "ok" : "warn"}>{r.complete ? "完整" : "不完整"}</Badge> },
    { key: "reason", header: "不可用原因", render: r => <>{blockers(r).length ? blockers(r).join("；") : <span className="muted">可选用</span>}{r.warnings.map(w => <span key={w}> <Badge tone="warn">{w}</Badge></span>)}</> },
    ...(mode === "multi" ? [{ key: "standard", header: "标准件", render: (r: HistoryCandidate) => <Checkbox label={<span className="qs-sr-only">{"SN " + r.sn + " 是标准件"}</span>} checked={picked[r.recordId] === true} disabled={disabled || working || !(r.recordId in picked)} onCheckedChange={on => setPicked(p => ({ ...p, [r.recordId]: on }))}/> }] : []),
    { key: "preview", header: "原图预览", render: r => <Button size="sm" variant="text" aria-label={"预览 SN " + r.sn} aria-pressed={previewId === r.recordId} disabled={!r.groupCount} onClick={() => setPreviewId(r.recordId)}>看图</Button> },
  ];
  const preview = rows?.find(r => r.recordId === previewId) ?? null;
  return <Panel title={title} titleHelp={help} actions={rows && <Badge tone={usable ? "ok" : "warn"}>{`可选用 ${usable} / ${rows.length} 件`}</Badge>}>
    {loadError && <Notice title="读取历史件失败" tone="warn">{loadError}</Notice>}
    <div className="wp-picker-table">
      <Table caption="历史件" columns={columns} rows={rows ?? []} rowKey={r => String(r.recordId)}
        getRowProps={r => r.recordId === previewId ? { tone: "info" } : undefined}
        emptyState={<p className="muted center">{rows ? "本配方还没有录制件。开启维护模式跟着生产采图后，可在这里选用。" : "正在读取历史件…"}</p>}/>
    </div>
    <div className="wp-actions"><Button variant="filled" disabled={disabled || working || !chosen} onClick={() => void confirm()}>{confirmLabel(chosen)}</Button></div>
    <aside className="wp-picker-preview" aria-label="历史件原图预览">
      {preview
        ? <FrameBrowser key={preview.recordId} source={String(preview.recordId)} label={`SN ${preview.sn}`} groupCount={preview.groupCount}
          cameras={preview.cameras.map(c => ({ id: c.cameraId, name: nameOf(c.cameraId) }))}
          read={(camera, seq) => burstImageApi.record(preview.recordId, camera, seq)} />
        : <p className="wp-capture-empty">勾选一件或点“看图”，在这里按相机、帧组查看录制原图。</p>}
    </aside>
  </Panel>;
}
