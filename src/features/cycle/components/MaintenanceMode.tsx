import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Wrench } from "lucide-react";
import { Alert, Button, Card, Dialog, TextField } from "qing-steel";
import { errorText } from "../../../lib/errors";
import { formatTime, verdictLabel } from "../../history/meta";
import type { HistoryFilter } from "../../history/types";
import { cycleApi, recipeApi } from "../api";
import type { CollectionProgress, Maintenance, RecipeSummary, TrialProgress } from "../types";

export function MaintenanceBanner({ maintenance }: { maintenance: Maintenance | null | undefined }) {
  if (!maintenance) return null;
  return <div className="qs-banner warn maintenance-banner" role="status" aria-label="维护模式">
    <span><strong>维护模式：不向 PLC 输出 NG</strong> · 每件向 PLC 回结果码 0，按未检放行 · 开启于 {formatTime(maintenance.since)} · 原因：{maintenance.reason}</span>
  </div>;
}

export function TrialProgressPanel({ trial, recipes }: { trial: TrialProgress[] | undefined; recipes: RecipeSummary[] }) {
  const go = useNavigate();
  if (!trial?.length) return null;
  return <Card className="maintenance-collection" role="region" aria-label="维护试判统计">
    <div className="panel-head">
      <h3 className="panel-title">维护试判</h3>
      <span className="spacer" />
      <span className="muted">结论不输出给 PLC；逐件复核 NG 与 ERR 后再退出维护模式</span>
    </div>
    <ul>{trial.map(item => {
      const name = recipes.find(recipe => recipe.id === item.recipeId)?.name;
      const { ok, excursion, ng, err } = item.counts;
      const split = item.ng.map(n => `${verdictLabel[n.verdict].replace("NG · ", "")} ${n.count}`).join("、");
      const filter: HistoryFilter = { recipeId: item.recipeId, modes: ["maintenanceShadow"], groups: ["ng", "err"] };
      return <li key={item.recipeId}>
        <b>{item.recipeId}</b> v{item.recipeVersion}{name && <span className="muted"> · {name}</span>}：试判 {ok + excursion + ng + err} 件 · OK {ok} · 局部超差 {excursion} · NG {ng}{split && `（${split}）`} · ERR {err}
        {ng + err > 0 && <Button size="sm" variant="text" onClick={() => go("/history", { state: { filter } })}>复核</Button>}
      </li>;
    })}</ul>
  </Card>;
}

export function CollectionProgressPanel({ collection, recipes }: { collection: CollectionProgress[] | undefined; recipes: RecipeSummary[] }) {
  if (!collection?.length) return null;
  return <Card className="maintenance-collection" role="region" aria-label="维护采集进度">
    <div className="panel-head">
      <h3 className="panel-title">维护采集进度</h3>
      <span className="spacer" />
      <span className="muted">标定至少需要 10 件，验证另需若干</span>
    </div>
    <ul>{collection.map(item => {
      const name = recipes.find(recipe => recipe.id === item.recipeId)?.name;
      return <li key={item.recipeId}><b>{item.recipeId}</b>{name && <span className="muted"> · {name}</span>}：已录 {item.recorded} 件，其中完整可用 {item.usable} 件</li>;
    })}</ul>
  </Card>;
}

type Pending = { enabled: boolean; collectOnly: RecipeSummary[] | null; listError: string };

export function MaintenanceToggle({ maintenance }: { maintenance: Maintenance | null | undefined }) {
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
    <Button variant={enabled ? "filled" : undefined} leadingIcon={<Wrench size={15} />} onClick={open}>{enabled ? "退出维护模式" : "维护模式"}</Button>
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
