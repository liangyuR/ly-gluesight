import { useEffect, useRef, useState } from "react";
import { Wrench } from "lucide-react";
import { Alert, Button, Dialog, Popover, TextField } from "qing-steel";
import { formatTime } from "../../history/meta";
import { errorText } from "../../../lib/errors";
import { cycleApi, recipeApi } from "../api";
import type { Maintenance, RecipeSummary } from "../types";

type Pending = { enabled: boolean; collectOnly: RecipeSummary[] | null; listError: string };

export function MaintenanceToggle({ maintenance, compact = false }: { maintenance: Maintenance | null | undefined; compact?: boolean }) {
  const [pending, setPending] = useState<Pending | null>(null);
  const [reason, setReason] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const request = useRef(0);
  useEffect(() => () => { request.current++; }, []);
  const enabled = !!maintenance;

  const open = () => {
    const id = ++request.current;
    setReason(""); setError("");
    if (!enabled) { setPending({ enabled: true, collectOnly: null, listError: "" }); return; }
    setPending({ enabled: false, collectOnly: null, listError: "" });
    void recipeApi.list().then(
      list => { if (request.current === id) setPending(p => p && { ...p, collectOnly: list.recipes.filter(recipe => recipe.stage === "collect") }); },
      e => { if (request.current === id) setPending(p => p && { ...p, collectOnly: [], listError: errorText(e) }); },
    );
  };
  const close = () => { if (busy) return; request.current++; setPending(null); };
  const confirm = async () => {
    if (!pending || busy || (pending.enabled && !reason.trim())) return;
    const id = ++request.current;
    setBusy(true); setError("");
    try {
      await cycleApi.setMaintenance(pending.enabled, pending.enabled ? reason.trim() : "");
      if (request.current === id) setPending(null);
    } catch (e) {
      if (request.current === id) setError(errorText(e));
    } finally {
      if (request.current === id) setBusy(false);
    }
  };

  return <>
    {compact && maintenance ? <Popover side="bottom" align="start" label="维护模式详情" title="维护模式" trigger={<button type="button" className="window-badge maintenance"><Wrench size={12} aria-hidden="true" />维护模式</button>}>
      <Alert tone="warn" title="不向 PLC 输出 NG">每件向 PLC 回结果码 0，按未检放行。</Alert>
      <p>开启于 {formatTime(maintenance.since)}</p>
      <p>原因：{maintenance.reason}</p>
      <Button onClick={open}>退出维护模式</Button>
    </Popover> : <Button size={compact ? "sm" : undefined} variant={enabled ? "filled" : "text"} leadingIcon={<Wrench size={compact ? 12 : 15} />} onClick={open}>{enabled ? "退出维护模式" : "维护模式"}</Button>}
    {pending && <Dialog
      open
      title={pending.enabled ? "开启维护模式" : "退出维护模式"}
      closeDisabled={busy}
      onOpenChange={next => { if (!next) close(); }}
      footer={<>
        {error && <span className="form-error" role="alert">{error}</span>}
        <Button disabled={busy} onClick={close}>取消</Button>
        <Button variant="filled" loading={busy} disabled={busy || (pending.enabled ? !reason.trim() : pending.collectOnly === null)} onClick={confirm}>{pending.enabled ? "确认开启" : "确认退出"}</Button>
      </>}
    >
      {pending.enabled ? <>
        <p>开启后每件照常采图，向 PLC 回结果码 0（按未检放行），不输出 NG；从下一件起生效，在途件按原模式完成。</p>
        <p>采集版配方只采图不判定；正式版配方照常判定（试判），结论不输出给 PLC。</p>
        <TextField label="开启原因" required value={reason} onChange={e => { setReason(e.target.value); setError(""); }} placeholder="如：新产线投运，采集示教与标定图像" />
      </> : <>
        <p>退出后从下一件起恢复正常检测，按判定结论向 PLC 输出。</p>
        {pending.collectOnly === null ? <p className="muted">正在核对配方库…</p>
          : pending.listError ? <Alert tone="warn">无法核对配方库：{pending.listError}。只有采集版的型号退出后会回检测错误（90/95）。</Alert>
            : pending.collectOnly.length > 0 && <Alert tone="warn" title="以下配方只有采集版、没有正式版" aria-label="只有采集版的配方">
              <ul>{pending.collectOnly.map(recipe => <li key={recipe.id}>{recipe.id} · {recipe.name}（PLC 型号 {recipe.productCode}）</li>)}</ul>
              退出后这些型号会回检测错误（90/95）。
            </Alert>}
      </>}
    </Dialog>}
  </>;
}
