import { useEffect, useRef, useState } from "react";
import { Tag, Trash2 } from "lucide-react";
import { Alert, Button, Chip, Dialog, Stack, Text, TextField } from "qing-steel";
import { errorText } from "../../../lib/errors";
import { historyApi } from "../api";
import type { HistoryDeleted } from "../types";

export const commonHistoryTags = ["故障件", "待复核", "误判件", "参考件"];

export function HistoryRecordDialog({ action, ids, initialTags, onClose, onSaved, onDeleted, onBusyChange }: {
  action: "tags" | "delete";
  ids: number[];
  initialTags: string[];
  onClose: () => void;
  onSaved: (tags: string[]) => void;
  onDeleted: (result: HistoryDeleted) => void;
  onBusyChange: (busy: boolean) => void;
}) {
  const [tags, setTags] = useState(initialTags);
  const [custom, setCustom] = useState("");
  const [error, setError] = useState("");
  const [running, setRunning] = useState(false);
  const [deleted, setDeleted] = useState<HistoryDeleted | null>(null);
  const pending = useRef(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; onBusyChange(false); }; }, [onBusyChange]);

  const addTag = () => {
    const next = custom.trim();
    if (!next) return;
    if ([...next].length > 24) { setError("每个 Tag 最多 24 个字符。"); return; }
    if (!tags.includes(next) && tags.length >= 8) { setError("每件最多设置 8 个 Tag。"); return; }
    setTags(tags.includes(next) ? tags : [...tags, next]);
    setCustom(""); setError("");
  };
  const toggleTag = (tag: string) => {
    if (!tags.includes(tag) && tags.length >= 8) { setError("每件最多设置 8 个 Tag。"); return; }
    setTags(tags.includes(tag) ? tags.filter(item => item !== tag) : [...tags, tag]);
    setError("");
  };
  const submit = async () => {
    if (pending.current || !ids.length) return;
    if (action === "tags" && custom.trim()) { setError("请先添加输入的自定义 Tag，再保存。"); return; }
    pending.current = true; setRunning(true); setError("");
    onBusyChange(true);
    let awaitingConfirmation = false;
    try {
      if (action === "delete") {
        const result = await historyApi.delete(ids);
        if (alive.current) {
          if (result.warnings.length) { awaitingConfirmation = true; setDeleted(result); }
          else onDeleted(result);
        }
      } else {
        const next = await historyApi.setTags(ids, tags);
        if (alive.current) onSaved(next);
      }
    } catch (cause) {
      if (alive.current) setError(errorText(cause));
    } finally {
      pending.current = false;
      if (!awaitingConfirmation) onBusyChange(false);
      if (alive.current) setRunning(false);
    }
  };

  if (deleted) return <Dialog open title="历史记录已删除" size="md" onOpenChange={open => { if (!open) onDeleted(deleted); }} footer={<Button variant="filled" onClick={() => onDeleted(deleted)}>完成</Button>}>
    <Stack direction="column" align="stretch" gap={12}>
      <Text as="p">已删除 {deleted.deleted} 件历史记录。</Text>
      <Alert tone="warn"><ul className="hist-package-list">{deleted.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></Alert>
    </Stack>
  </Dialog>;

  return <Dialog open title={action === "delete" ? "删除历史件" : "编辑 Tag"} size="md" closeDisabled={running}
    onOpenChange={open => { if (!open && !running) onClose(); }}
    footer={<><Button disabled={running} onClick={onClose}>取消</Button><Button variant="filled" loading={running} disabled={running} onClick={() => void submit()}>{running ? "正在处理…" : action === "delete" ? `确认删除 ${ids.length} 件` : "保存 Tag"}</Button></>}>
    <Stack direction="column" align="stretch" gap={12}>
      {action === "delete" ? <>
        <Alert tone="warn">确定删除所选 {ids.length} 件历史记录？删除后不可恢复，相关帧记录、人工 Tag 和可安全清理的关联原图也会删除。</Alert>
        <Text as="p" muted>正在处理或仍有待交付数据的记录不能删除。共享原图和配方资料按后端校验保留。</Text>
      </> : <>
        <Text as="p">为所选 {ids.length} 件设置人工 Tag。Tag 不改变原始检测结论。</Text>
        {ids.length > 1 && <Alert tone="warn">保存将用下方 Tag 替换所有选中件的原有 Tag。</Alert>}
        <Stack gap={8} className="hist-tag-options">
          {[...new Set([...commonHistoryTags, ...tags])].map(tag => <Chip key={tag} selected={tags.includes(tag)} disabled={running} onClick={() => toggleTag(tag)}>{tag}</Chip>)}
        </Stack>
        <Stack gap={8}>
          <TextField aria-label="自定义 Tag" placeholder="输入自定义 Tag，最多 24 个字符" value={custom} disabled={running} onChange={event => setCustom(event.target.value)} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); addTag(); } }} />
          <Button disabled={running || !custom.trim()} onClick={addTag}>添加 Tag</Button>
        </Stack>
        <Text variant="label" muted>已选 {tags.length}/8 个。取消全部选择后保存，可清空 Tag。</Text>
      </>}
      {error && <Alert tone="ng">{error}</Alert>}
    </Stack>
  </Dialog>;
}

export function HistoryRecordActions({ id, tags, onSaved, onDeleted, onBusyChange }: { id: number; tags: string[]; onSaved: (tags: string[]) => void; onDeleted: (result: HistoryDeleted) => void; onBusyChange: (busy: boolean) => void }) {
  const [action, setAction] = useState<"tags" | "delete" | null>(null);
  return <>
    <div className="hist-record-actions">
      <Stack gap={8}><Text variant="label" muted>人工 Tag</Text>{tags.length ? tags.map(tag => <Chip key={tag}>{tag}</Chip>) : <Text muted>未标记</Text>}</Stack>
      <Stack gap={8}>
        <Button leadingIcon={<Tag size={15} />} onClick={() => setAction("tags")}>编辑 Tag</Button>
        <Button leadingIcon={<Trash2 size={15} />} onClick={() => setAction("delete")}>删除历史件</Button>
      </Stack>
    </div>
    {action && <HistoryRecordDialog action={action} ids={[id]} initialTags={tags} onClose={() => setAction(null)} onSaved={next => { setAction(null); onSaved(next); }} onDeleted={onDeleted} onBusyChange={onBusyChange} />}
  </>;
}
