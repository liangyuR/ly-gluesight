import { useRef, useState } from "react";
import { Alert, Button, Chip, Dialog, NumberField, Select, Stack, Text } from "qing-steel";
import { fromLocalInput, toLocalInput } from "../plc/time";
import type { HistoryScope } from "../history/types";
import { errorText } from "../../lib/errors";
import { formatBytes, logsApi } from "./api";
import DateTimeField from "./DateTimeField";
import type { DiagnosticsResult, ImageMode } from "./types";

const presets = [["1 小时", 3_600_000], ["24 小时", 86_400_000], ["7 天", 7 * 86_400_000]] as const;
const scopes: [HistoryScope | "all", string][] = [["all", "全部记录"], ["real", "真机件"], ["offline", "离线件"], ["debug", "候选调试"]];
const imageModes: [ImageMode, string][] = [["abnormal", "仅异常件（NG / ERR）"], ["all", "全部工件"], ["none", "不含原图"]];

export default function DiagnosticsDialog({ from, to, historyIds, title = "导出诊断包", onClose }: {
  from?: number; to?: number; historyIds?: number[]; title?: string; onClose: () => void;
}) {
  const now = Date.now();
  const [start, setStart] = useState(() => toLocalInput(from ?? now - 3_600_000));
  const [end, setEnd] = useState(() => toLocalInput(to ?? now));
  const [scope, setScope] = useState<HistoryScope | "all">("all");
  const [images, setImages] = useState<ImageMode>(historyIds?.length ? "all" : "abnormal");
  const [maxParts, setMaxParts] = useState<number | "">(20);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<DiagnosticsResult | null>(null);
  const alive = useRef(true);
  const named = !!historyIds?.length;
  const run = async () => {
    const a = fromLocalInput(start), b = fromLocalInput(end);
    if (a === null || b === null || a >= b) { setError("请选择有效时间段，开始时间需早于结束时间"); return; }
    if (typeof maxParts !== "number" || !Number.isInteger(maxParts) || maxParts < 1 || maxParts > 200) { setError("附原图件数需为 1–200 的整数"); return; }
    setBusy(true); setError(""); setResult(null);
    try {
      const done = await logsApi.exportDiagnostics({ from: a, to: b, scope: scope === "all" ? null : scope, historyIds: historyIds ?? [], images, maxImageParts: maxParts });
      if (alive.current) setResult(done);
    } catch (e) { if (alive.current) setError(errorText(e)); }
    finally { if (alive.current) setBusy(false); }
  };
  const close = () => { alive.current = false; onClose(); };
  return <Dialog open onOpenChange={open => { if (!open) close(); }} title={title} className="diag-dialog" closeDisabled={busy}
    description={`诊断包包含时间段内的运行日志、PLC 日志、握手审计、配置与状态文件、检测记录（CSV 与完整明细）、所用配方快照${named ? "，以及所选记录的原图与对照结果" : "，可选附带原图与对照结果"}。包内 README.md 说明包结构、时间口径（ts 为毫秒 Unix 时间戳，t 为带时区的本地时间）与推荐分析顺序，manifest.json 记录构建版本、配置摘要与文件校验值。导出到资料目录的 exports/。`}
    footer={<>
      <Button disabled={busy} onClick={close}>{result ? "完成" : "取消"}</Button>
      <Button variant="filled" loading={busy} onClick={() => void run()}>{busy ? "正在打包…" : "导出"}</Button>
    </>}>
    <Stack direction="column" align="stretch" gap={12}>
      <Stack gap={8}>
        <Text variant="label" muted>日志时间</Text>
        <DateTimeField label="诊断包开始" value={start} disabled={busy} onChange={setStart} />
        <Text muted>至</Text>
        <DateTimeField label="诊断包结束" value={end} disabled={busy} onChange={setEnd} />
      </Stack>
      {!named && <Stack gap={8}><Text variant="label" muted>最近</Text>{presets.map(([label, ms]) => <Chip key={label} disabled={busy} onClick={() => { const t = Date.now(); setStart(toLocalInput(t - ms)); setEnd(toLocalInput(t)); }}>{label}</Chip>)}</Stack>}
      {named ? <Text as="p" muted>记录：{historyIds!.map(id => "#" + id).join("、")}</Text> :
        <Select label="检测记录" aria-label="诊断包记录范围" value={scope} disabled={busy} onValueChange={value => setScope(value as HistoryScope | "all")} options={scopes.map(([value, label]) => ({ value, label }))} />}
      <Select label="原图" aria-label="诊断包原图" value={images} disabled={busy} onValueChange={value => setImages(value as ImageMode)}
        options={imageModes.filter(([key]) => !named || key !== "abnormal").map(([value, label]) => ({ value, label: named && value === "all" ? "所选记录的原图" : label }))} />
      {!named && images !== "none" && <NumberField label="最多附原图件数（新的优先，原图总量上限 2 GB）" aria-label="最多附原图件数" min={1} max={200} value={maxParts} disabled={busy} onValueChange={setMaxParts} />}
      {error && <Alert tone="ng">{error}</Alert>}
      {result && <Alert tone="ok" action={<Button variant="text" size="sm" onClick={() => void logsApi.reveal(result.path).catch(e => setError(errorText(e)))}>打开所在文件夹</Button>}>
        <div>已导出 <span className="mono">{result.path}</span>（{formatBytes(result.bytes)}）</div>
        <div>运行日志 {result.logs} 条 · PLC 日志 {result.plcLogs} 条 · 检测记录 {result.parts} 件 · 原图 {result.images} 幅（{result.imageParts} 件）</div>
      </Alert>}
      {!!result?.notes.length && <details><summary>打包说明（{result.notes.length}）</summary><ul className="diag-notes">{result.notes.map((note, i) => <li key={i}>{note}</li>)}</ul></details>}
    </Stack>
  </Dialog>;
}
