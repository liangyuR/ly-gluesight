import { useRef, useState } from "react";
import { Button, Dialog } from "qing-steel";
import { errorText } from "../../lib/errors";
import { formatBytes, formatDateTime } from "../project/rules";
import { workspaceApi } from "./api";
import { Notice } from "./components";
import { pieceLabel } from "./pieces";
import type { PackageExported, PackageImported, PackagePreview } from "./types";

export function PackageExportedDialog({ id, exported, onClose }: { id: string; exported: PackageExported; onClose: () => void }) {
  return <Dialog open title="已导出配方回传包" onOpenChange={open => { if (!open) onClose(); }} footer={<Button variant="filled" onClick={onClose}>完成</Button>}>
    <div className="wp-stack">
      <dl className="kv">
        <dt>配方</dt><dd className="mono">{id}</dd>
        <dt>保存位置</dt><dd className="mono">{exported.path}</dd>
        <dt>大小</dt><dd>{formatBytes(exported.bytes)} · {exported.files} 个文件</dd>
      </dl>
      <p className="muted">包里是候选（分段、走廊、取色、示教件测量、导航图）与示教件的帧记录，不带原图。在现场软件的配方库点“导入回传包”，示教件原图从现场的历史记录补回。</p>
    </div>
  </Dialog>;
}

export function PackageImportDialog({ preview, onClose, onImported, onOpen }: { preview: PackagePreview; onClose: () => void; onImported: (id: string) => Promise<void>; onOpen: (id: string) => void }) {
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
      const result = await workspaceApi.packageImport(preview.path);
      setImported(result);
      await onImported(result.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      pending.current = false;
      setWorking(false);
    }
  };
  if (imported) return <Dialog open title="已导入配方回传包" onOpenChange={open => { if (!open) onClose(); }}
    footer={<><Button onClick={onClose}>完成</Button><Button variant="filled" onClick={() => onOpen(imported.id)}>打开候选</Button></>}>
    <div className="wp-stack">
      <p>候选 <span className="mono">{imported.id}</span> 已导入，处于草稿状态；在本机发布后才用于检测。</p>
      {imported.notes.length > 0 && <ul>{imported.notes.map(note => <li key={note}>{note}</li>)}</ul>}
    </div>
  </Dialog>;
  const local = preview.candidate;
  return <Dialog open title="导入配方回传包" size="md" closeDisabled={working} onOpenChange={open => { if (!open && !working) onClose(); }}
    footer={<><Button disabled={working} onClick={onClose}>取消</Button><Button variant="filled" loading={working} disabled={working || !!preview.blocker} onClick={() => void submit()}>{working ? "正在导入…" : local ? "导入并替换候选" : "导入候选"}</Button></>}>
    <div className="wp-stack">
      <dl className="kv">
        <dt>配方</dt><dd>{preview.name} <span className="mono">{preview.recipeId}</span> · 型号 {preview.productCode}</dd>
        <dt>示教件</dt><dd>{preview.teachingPiece ? pieceLabel(preview.teachingPiece) : "未选示教件"}</dd>
        <dt>来源项目</dt><dd>{preview.sourceProjectName}</dd>
        <dt>导出时间</dt><dd className="mono">{formatDateTime(preview.exportedAt)}</dd>
        <dt>软件版本</dt><dd className="mono">{preview.appVersion || "未知"}</dd>
        <dt>大小</dt><dd>{formatBytes(preview.bytes)}</dd>
        <dt>本机生产配方</dt><dd>{preview.productionVersion != null ? `v${preview.productionVersion}，发布前照常检测` : "没有，导入后是新配方的候选"}</dd>
      </dl>
      {local && <Notice tone="warn" title="将替换本机候选">本机已有候选 {preview.recipeId}（更新于 {formatDateTime(local.updatedAt)}），导入后被包里的候选替换，它的分段、走廊与示教件不保留；本机的发布记录不受影响。</Notice>}
      <Notice title="导入后">候选回到草稿，基于本机的生产配方；验证样本不随包导入。示教件原图按示教件那一件从本机历史记录补回，那一件的录制已被清理时需要重新选用示教件。</Notice>
      {preview.blocker && <Notice tone="warn" title="不能导入">{preview.blocker}</Notice>}
      {error && <Notice tone="warn" title="导入未完成">{error}</Notice>}
    </div>
  </Dialog>;
}
