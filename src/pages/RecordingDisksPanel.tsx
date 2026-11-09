import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Alert, Button, Card, Heading, IconButton, Stack, StatusBadge, TextField, useSnackbar } from "qing-steel";
import { storageApi, type RecordingDisks } from "../lib/storage";
import { errorText } from "../lib/errors";

const gb = (bytes: number) => `${(bytes / 1024 ** 3).toFixed(1)} GB`;
const key = (path: string) => path.trim().replace(/\//g, "\\").replace(/^\\\\\?\\/, "").replace(/\\+$/, "").toLowerCase();

function displayPath(path: string): string {
  if (/^\\\\\?\\UNC\\/i.test(path)) return `\\\\${path.slice(8)}`;
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) return path.slice(4);
  return path;
}

export default function RecordingDisksPanel() {
  const [data, setData] = useState<RecordingDisks | null>(null);
  const [draft, setDraft] = useState<string[]>([]);
  const [adding, setAdding] = useState("");
  const [loadError, setLoadError] = useState("");
  const [saveError, setSaveError] = useState("");
  const [saving, setSaving] = useState(false);
  const mounted = useRef(true);
  const { show: snack } = useSnackbar();

  const load = useCallback(() => {
    setLoadError("");
    storageApi.recordingDisks().then((value) => {
      if (!mounted.current) return;
      setData(value);
      setDraft(value.disks.map((disk) => disk.path));
    }).catch((e) => { if (mounted.current) setLoadError(errorText(e)); });
  }, []);
  useEffect(() => {
    mounted.current = true;
    load();
    return () => { mounted.current = false; };
  }, [load]);

  const saved = data?.disks.map((disk) => disk.path) ?? [];
  const dirty = draft.join("\n") !== saved.join("\n");
  const addError = adding.trim() && draft.some((path) => key(path) === key(adding)) ? "这块盘已在列表里" : "";
  const move = (from: number, to: number) => setDraft((list) => {
    const next = [...list];
    const [item] = next.splice(from, 1);
    next.splice(to, 0, item);
    return next;
  });
  const add = () => {
    if (!adding.trim() || addError) return;
    setDraft((list) => [...list, adding.trim()]);
    setAdding("");
  };
  const save = async () => {
    if (saving || draft.length === 0) return;
    setSaving(true);
    setSaveError("");
    try {
      const value = await storageApi.saveRecordingDisks(draft);
      if (!mounted.current) return;
      setData(value);
      setDraft(value.disks.map((disk) => disk.path));
      snack("录制盘已保存");
    } catch (e) {
      if (mounted.current) setSaveError(errorText(e));
    } finally {
      if (mounted.current) setSaving(false);
    }
  };

  return <Card role="region" aria-label="录制盘" title={<Heading level={3}>录制盘</Heading>}
    actions={<IconButton aria-label="刷新录制盘" title="刷新" onClick={load}><RefreshCw size={16} /></IconButton>}>
    <Stack direction="column" align="stretch" gap={12} wrap={false}>
      <p className="muted">
        新件按顺序写到第一块剩余空间不少于 {data ? gb(data.reserveBytes) : "5 GB"} 的盘，一件只写一块盘；所有盘都不足时告警、照常检测、不录原图。
        已录的件按记录里的目录读，移除或离线的盘上的原图看不了，加回或接上后可读。
      </p>
      {loadError && <Alert tone="ng" action={<Button size="sm" onClick={load}>重试</Button>}>录制盘读取失败：{loadError}</Alert>}
      {data && <ol className="settings-disk-list" aria-label="录制盘顺序">
        {draft.map((path, i) => {
          const info = data.disks.find((disk) => disk.path === path);
          return <li key={path} className="settings-disk-row">
            <span className="mono settings-disk-path">{i + 1}. {displayPath(path)}</span>
            <span className="settings-disk-state">
              {!info ? <span className="muted">保存时检查</span> : <>
                {info.current && <StatusBadge tone="ok">新件写这里</StatusBadge>}
                {info.writable
                  ? <span className={info.freeBytes != null && info.freeBytes < data.reserveBytes ? "ng" : undefined}>
                    {info.freeBytes == null ? "剩余空间未知" : `剩余 ${gb(info.freeBytes)}`}
                  </span>
                  : <span className="ng">不可写：{info.error ?? "原因不明"}</span>}
              </>}
            </span>
            <span className="settings-disk-actions">
              <IconButton aria-label={`上移 ${displayPath(path)}`} title="上移" disabled={i === 0} onClick={() => move(i, i - 1)}><ArrowUp size={16} /></IconButton>
              <IconButton aria-label={`下移 ${displayPath(path)}`} title="下移" disabled={i === draft.length - 1} onClick={() => move(i, i + 1)}><ArrowDown size={16} /></IconButton>
              <IconButton aria-label={`移除 ${displayPath(path)}`} title="移除" disabled={draft.length === 1} onClick={() => setDraft((list) => list.filter((_, j) => j !== i))}><Trash2 size={16} /></IconButton>
            </span>
          </li>;
        })}
      </ol>}
      {data && <Stack gap={8}>
        <TextField aria-label="新增录制盘路径" className="settings-disk-input" placeholder="例如 D:\gs_logs" value={adding}
          onChange={(e) => setAdding(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") add(); }} />
        <Button size="sm" onClick={add} disabled={!adding.trim() || !!addError}><Plus size={16} />添加</Button>
        <Button size="sm" variant="filled" onClick={() => void save()} disabled={!dirty || saving || draft.length === 0}>{saving ? "保存中…" : "保存录制盘"}</Button>
        {dirty && <Button size="sm" variant="text" onClick={() => setDraft(saved)}>撤销修改</Button>}
      </Stack>}
      {addError && <Alert tone="warn">{addError}</Alert>}
      {saveError && <Alert tone="ng">保存失败：{saveError}</Alert>}
    </Stack>
  </Card>;
}
