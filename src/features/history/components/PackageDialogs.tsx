import { useRef, useState } from "react";
import { Alert, Button, Checkbox, Dialog, Stack, Text } from "qing-steel";
import { errorText } from "../../../lib/errors";
import { formatBytes, formatDateTime } from "../../project/rules";
import { OperationStatus, useOperation, type OperationLabels } from "../../workspace/operation";
import { historyApi } from "../api";
import { formatTime } from "../meta";
import type { HistoryPackageExported, HistoryPackageImported, HistoryPackagePreview } from "../types";

const MAX_PARTS = 20;
const exportLabels: OperationLabels = { measuring: "正在打包", saving: "正在写入导出包", cancelling: "正在取消，等待当前文件结束" };
const importLabels: OperationLabels = { measuring: "正在解包校验", saving: "正在写入历史", cancelling: "正在取消，等待当前文件结束" };

function Notes({ notes }: { notes: string[] }) {
  if (!notes.length) return null;
  return <ul className="hist-package-list">{notes.map((note, i) => <li key={i}>{note}</li>)}</ul>;
}

export function HistoryExportDialog({ ids, onClose }: { ids: number[]; onClose: () => void }) {
  const operation = useOperation();
  const [includeImages, setIncludeImages] = useState(true);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const [exported, setExported] = useState<HistoryPackageExported | null>(null);
  const [revealing, setRevealing] = useState(false);
  const pending = useRef(false);
  const tooMany = ids.length > MAX_PARTS;

  const submit = async () => {
    if (pending.current || tooMany || !ids.length) return;
    pending.current = true;
    setRunning(true);
    setError("");
    try {
      const path = await historyApi.packageTarget(ids.length);
      if (!path) return;
      const result = await operation.run(taskId => historyApi.packageExport(ids, includeImages, path, taskId)).catch(() => null);
      if (result) setExported(result);
    } catch (e) {
      setError(errorText(e));
    } finally {
      pending.current = false;
      setRunning(false);
    }
  };

  const reveal = async () => {
    if (!exported || revealing) return;
    setRevealing(true);
    setError("");
    try { await historyApi.reveal(exported.path); }
    catch (e) { setError(errorText(e)); }
    finally { setRevealing(false); }
  };

  if (exported) return <Dialog open title="已导出历史件" size="md" onOpenChange={open => { if (!open) onClose(); }}
    footer={<><Button disabled={revealing} onClick={() => void reveal()}>{revealing ? "正在打开…" : "打开所在文件夹"}</Button><Button variant="filled" onClick={onClose}>完成</Button></>}>
    <Stack direction="column" align="stretch" gap={12}>
      <dl className="kv">
        <dt>保存位置</dt><dd className="mono hist-package-path">{exported.path}</dd>
        <dt>大小</dt><dd>{formatBytes(exported.bytes)}</dd>
        <dt>件数</dt><dd>{exported.parts} 件，其中 {exported.withImages} 件带原图</dd>
      </dl>
      <Notes notes={exported.notes} />
      {error && <Alert tone="ng">{error}</Alert>}
    </Stack>
  </Dialog>;

  return <Dialog open title="导出历史件" size="md" closeDisabled={running} onOpenChange={open => { if (!open && !running) onClose(); }}
    footer={<><Button disabled={running} onClick={onClose}>关闭</Button><Button variant="filled" loading={running} disabled={running || tooMany || !ids.length} onClick={() => void submit()}>{running ? "正在导出…" : `导出 ${ids.length} 件`}</Button></>}>
    <Stack direction="column" align="stretch" gap={12}>
      <Text as="p">已选 <b>{ids.length}</b> 件，打包成一个 .zip，可在另一台电脑的历史记录页导入。</Text>
      <Checkbox label="包含原图" checked={includeImages} disabled={running} onCheckedChange={setIncludeImages} />
      <Text as="p" variant="label" muted>每件原图约 0.7–5 GB；不带原图时导入方只能按已存指标重判，不能原图复测或选作示教件。</Text>
      {tooMany && <Alert tone="warn">一次最多导出 {MAX_PARTS} 件，请减少选择。</Alert>}
      <OperationStatus operation={operation} label="导出历史件" labels={exportLabels} unit="个文件" />
      {error && <Alert tone="ng">{error}</Alert>}
    </Stack>
  </Dialog>;
}

export function HistoryImportDialog({ preview, projectName, onClose, onImported }: { preview: HistoryPackagePreview; projectName?: string; onClose: () => void; onImported: () => void }) {
  const operation = useOperation();
  const [running, setRunning] = useState(false);
  const [imported, setImported] = useState<HistoryPackageImported | null>(null);
  const pending = useRef(false);

  const submit = async () => {
    if (pending.current) return;
    pending.current = true;
    setRunning(true);
    const result = await operation.run(taskId => historyApi.packageImport(preview.path, taskId)).catch(() => null);
    pending.current = false;
    setRunning(false);
    if (result) { setImported(result); onImported(); }
  };

  if (imported) return <Dialog open title="已导入历史件" size="md" onOpenChange={open => { if (!open) onClose(); }} footer={<Button variant="filled" onClick={onClose}>完成</Button>}>
    <Stack direction="column" align="stretch" gap={12}>
      {imported.imported > 0
        ? <Text as="p">已导入 <b>{imported.imported}</b> 件，归当前项目。</Text>
        : <Alert tone="warn">包里的件本机都已有，没有导入新件。</Alert>}
      {imported.skipped.length > 0 && <>
        <Text as="p" variant="label" muted>本机已有、跳过 {imported.skipped.length} 件：</Text>
        <ul className="hist-package-list hist-package-skipped" aria-label="跳过的重复件">
          {imported.skipped.map(item => <li key={item.cycleId}><span className="sn">SN {item.sn || "—"}</span> · <span className="mono">{formatTime(item.ts)}</span></li>)}
        </ul>
      </>}
      <Notes notes={imported.notes} />
    </Stack>
  </Dialog>;

  return <Dialog open title="导入历史件" size="md" closeDisabled={running} onOpenChange={open => { if (!open && !running) onClose(); }}
    footer={<><Button disabled={running} onClick={onClose}>取消</Button><Button variant="filled" loading={running} disabled={running} onClick={() => void submit()}>{running ? "正在导入…" : "导入"}</Button></>}>
    <Stack direction="column" align="stretch" gap={12}>
      <dl className="kv">
        <dt>来源项目</dt><dd>{preview.sourceProjectName} <span className="mono muted">{preview.sourceProject}</span></dd>
        <dt>导出时间</dt><dd className="mono">{formatDateTime(preview.exportedAt)}</dd>
        <dt>软件版本</dt><dd className="mono">{preview.appVersion || "未知"}</dd>
        <dt>件数</dt><dd>{preview.parts} 件，其中 {preview.withImages} 件带原图</dd>
        <dt>本机已有</dt><dd>{preview.duplicates > 0 ? `${preview.duplicates} 件，导入时跳过` : "没有"}</dd>
        <dt>包大小</dt><dd>{formatBytes(preview.bytes)}</dd>
      </dl>
      <Notes notes={preview.notes} />
      <Alert tone="info">导入的件归当前项目{projectName ? `（${projectName}）` : ""}。</Alert>
      <OperationStatus operation={operation} label="导入历史件" labels={importLabels} unit="个文件" />
    </Stack>
  </Dialog>;
}
