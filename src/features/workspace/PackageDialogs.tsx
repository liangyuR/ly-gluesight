import { useRef, useState } from "react";
import { Button, Checkbox, Dialog } from "qing-steel";
import { errorText } from "../../lib/errors";
import { formatBytes, formatDateTime } from "../project/rules";
import { workspaceApi } from "./api";
import { Notice } from "./components";
import { OperationStatus, useOperation, type OperationLabels } from "./operation";
import { pieceLabel } from "./pieces";
import type { PackageExported, PackageImported, PackagePreview } from "./types";

const exportLabels: OperationLabels = { measuring: "正在打包", saving: "正在写入工件配方包", cancelling: "正在取消，等待当前文件结束" };
const importLabels: OperationLabels = { measuring: "正在解包校验", saving: "正在替换候选", cancelling: "正在取消，等待当前文件结束" };
const IMAGES_MISSING = "示教件原图不在本机时，看示教图、示教走廊、测量示教件和试测都不可用；分段、走廊、取色与示教件测量结果照常导入，候选仍可发布。";

function Notes({ notes }: { notes: string[] }) {
  return notes.length > 0 ? <ul>{notes.map(note => <li key={note}>{note}</li>)}</ul> : null;
}

export function PackageExportDialog({ id, onClose }: { id: string; onClose: () => void }) {
  const operation = useOperation();
  const [includeImages, setIncludeImages] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const [exported, setExported] = useState<PackageExported | null>(null);
  const pending = useRef(false);
  const submit = async () => {
    if (pending.current) return;
    pending.current = true;
    setRunning(true);
    setError("");
    try {
      const path = await workspaceApi.packageTarget(id);
      if (!path) return;
      const result = await operation.run(taskId => workspaceApi.packageExport(id, includeImages, path, taskId)).catch(() => null);
      if (result) setExported(result);
    } catch (e) {
      setError(errorText(e));
    } finally {
      pending.current = false;
      setRunning(false);
    }
  };
  if (exported) return <Dialog open title="已导出工件配方" onOpenChange={open => { if (!open) onClose(); }} footer={<Button variant="filled" onClick={onClose}>完成</Button>}>
    <div className="wp-stack">
      <dl className="kv">
        <dt>配方</dt><dd className="mono">{id}</dd>
        <dt>保存位置</dt><dd className="mono">{exported.path}</dd>
        <dt>大小</dt><dd>{formatBytes(exported.bytes)} · {exported.files} 个文件</dd>
        <dt>示教件原图</dt><dd>{exported.teachingImages ? "已带上" : "未带"}</dd>
      </dl>
      <p className="muted">{exported.teachingImages
        ? "在目标软件的配方库点“导入工件配方”，导入后可直接看示教图与试测。"
        : "在目标软件的配方库点“导入工件配方”，示教件原图从那台机器的历史记录补回；那里没有示教件那一件的录制时，看示教图、示教走廊、测量示教件和试测不可用。"}</p>
    </div>
  </Dialog>;
  return <Dialog open title="导出工件配方" size="md" closeDisabled={running} onOpenChange={open => { if (!open && !running) onClose(); }}
    footer={<><Button disabled={running} onClick={onClose}>取消</Button><Button variant="filled" loading={running} disabled={running} onClick={() => void submit()}>{running ? "正在导出…" : "选择位置并导出"}</Button></>}>
    <div className="wp-stack">
      <p>把候选 <span className="mono">{id}</span> 的分段、走廊、取色、示教件测量、导航图与示教件帧记录打成一个 .zip，可导入本机或另一台机器的当前项目。</p>
      <Checkbox label="包含示教件原图" checked={includeImages} disabled={running} onCheckedChange={setIncludeImages} />
      <p className="muted">{includeImages
        ? "示教件原图较大（每件几百 MB 到数 GB），导入后可直接看示教图与试测，适合目标机器没有示教件那一件录制的情况。"
        : `不带原图时包只有几 MB，导入后从目标机器的历史记录补回示教件原图（适合送回示教件所在的现场项目）。${IMAGES_MISSING}`}</p>
      <OperationStatus operation={operation} label="导出工件配方" labels={exportLabels} unit="个文件" />
      {error && <Notice tone="warn" title="导出未完成">{error}</Notice>}
    </div>
  </Dialog>;
}

export function PackageImportDialog({ preview, onClose, onImported, onOpen }: { preview: PackagePreview; onClose: () => void; onImported: (id: string) => Promise<void>; onOpen: (id: string) => void }) {
  const operation = useOperation();
  const [working, setWorking] = useState(false);
  const [error, setError] = useState("");
  const [imported, setImported] = useState<PackageImported | null>(null);
  const pending = useRef(false);
  const submit = async () => {
    if (pending.current || preview.blocker) return;
    pending.current = true;
    setWorking(true);
    setError("");
    try {
      const result = await operation.run(taskId => workspaceApi.packageImport(preview.path, taskId)).catch(() => null);
      if (!result) return;
      setImported(result);
      await onImported(result.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      pending.current = false;
      setWorking(false);
    }
  };
  if (imported) return <Dialog open title="已导入工件配方" onOpenChange={open => { if (!open) onClose(); }}
    footer={<><Button onClick={onClose}>完成</Button><Button variant="filled" onClick={() => onOpen(imported.id)}>打开候选</Button></>}>
    <div className="wp-stack">
      <p>候选 <span className="mono">{imported.id}</span> 已导入，处于草稿状态；在本机发布后才用于检测。</p>
      {!imported.teachingImages && <Notice tone="warn" title="示教件原图缺失">{IMAGES_MISSING}需要时在示教件页补回原图，或重新选用示教件。</Notice>}
      <Notes notes={imported.notes} />
    </div>
  </Dialog>;
  const local = preview.candidate;
  return <Dialog open title="导入工件配方" size="md" closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button variant="filled" loading={working} disabled={working || !!preview.blocker} onClick={() => void submit()}>{working ? "正在导入…" : local ? "导入并替换候选" : "导入候选"}</Button></>}>
    <div className="wp-stack">
      <dl className="kv">
        <dt>配方</dt><dd>{preview.name} <span className="mono">{preview.recipeId}</span> · 型号 {preview.productCode}</dd>
        <dt>示教件</dt><dd>{preview.teachingPiece ? pieceLabel(preview.teachingPiece) : "未选示教件"}</dd>
        <dt>示教件原图</dt><dd>{preview.teachingImages ? "随包带入，导入后可直接试测" : "未带，导入后从本机历史记录补回"}</dd>
        <dt>来源项目</dt><dd>{preview.sourceProjectName}</dd>
        <dt>导出时间</dt><dd className="mono">{formatDateTime(preview.exportedAt)}</dd>
        <dt>软件版本</dt><dd className="mono">{preview.appVersion || "未知"}</dd>
        <dt>大小</dt><dd>{formatBytes(preview.bytes)}</dd>
        <dt>本机生产配方</dt><dd>{preview.productionVersion != null ? `v${preview.productionVersion}，发布前照常检测` : "没有，导入后是新配方的候选"}</dd>
      </dl>
      {local && <Notice tone="warn" title="将替换本机候选">本机已有候选 {preview.recipeId}（更新于 {formatDateTime(local.updatedAt)}），导入后被包里的候选替换，它的分段、走廊与示教件不保留；本机的发布记录不受影响。</Notice>}
      {preview.warnings.length > 0 && <Notice tone="warn" title="相机与示教件采集时不一致"><Notes notes={preview.warnings} /></Notice>}
      <Notice title="导入后">候选回到草稿，版本号按本机分配、基于本机的生产配方；验证样本不随包导入。{preview.teachingImages ? "" : `示教件那一件不在本机历史记录里（录制已清理或来自别的机器）时补不回原图：${IMAGES_MISSING}`}</Notice>
      {preview.blocker && <Notice tone="warn" title="不能导入">{preview.blocker}</Notice>}
      <OperationStatus operation={operation} label="导入工件配方" labels={importLabels} unit="个文件" />
      {error && <Notice tone="warn" title="导入未完成">{error}</Notice>}
    </div>
  </Dialog>;
}
