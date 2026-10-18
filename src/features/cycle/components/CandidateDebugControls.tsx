import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { errorText } from "../../../lib/errors";
import { workspaceApi } from "../../workspace/api";
import type { Workspace } from "../../workspace/types";
import type { SiteStatus, Snapshot } from "../types";
import SamplePartSelect from "./SamplePartSelect";

export default function CandidateDebugControls({ site, snapshot }: { site: SiteStatus | null; snapshot: Snapshot | null }) {
  const [search] = useSearchParams();
  const [items, setItems] = useState<Workspace[]>([]);
  const [selected, setSelected] = useState(search.get("candidate") ?? "");
  const [partId, setPartId] = useState("");
  const [loading, setLoading] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const pending = useRef(false);
  const generation = useRef(0);
  const available = site?.mode === "offline" && site.source === "sampleSet";
  const setId = site?.sampleSet?.id;
  useEffect(() => {
    const request = ++generation.current;
    setItems([]); setError(""); setPartId(""); setStarting(false); pending.current = false;
    if (!available) { setLoading(false); return; }
    setLoading(true);
    void workspaceApi.list().then(values => {
      if (generation.current !== request) return;
      setItems(values);
      setSelected(previous => values.some(w => w.doc.id === previous) ? previous : values[0]?.doc.id ?? "");
    }).catch(e => { if (generation.current === request) setError(errorText(e)); })
      .finally(() => { if (generation.current === request) setLoading(false); });
    return () => { generation.current++; };
  }, [available, setId, refresh]);
  if (!available || !site.sampleSet) return null;
  const candidate = items.find(w => w.doc.id === selected);
  const idle = !!snapshot && !snapshot.plcLocked && ["IDLE", "FAULT"].includes(snapshot.phase);
  const locked = loading || starting || !!site.running || !idle || !!snapshot?.candidateDebug;
  const run = async () => {
    if (locked || pending.current || !candidate || candidate.pending) return;
    const request = generation.current;
    pending.current = true; setStarting(true); setError("");
    try { await workspaceApi.debugRun(candidate.doc.id, candidate.revision, partId || null); }
    catch(e) { if (generation.current === request) setError(errorText(e)); }
    finally { if (generation.current === request) { pending.current = false; setStarting(false); } }
  };
  return <section className="panel" aria-label="候选配方离线试运行">
    <details open={search.has("candidate")}>
      <summary>候选配方离线试运行 · 无需发布</summary>
      <p className="muted">使用已保存的候选跑一圈。结果单独标记为调试，不替代标定、独立样本验证或生产发布。</p>
      <div className="wp-actions">
        <select className="input" aria-label="调试候选配方" value={selected} disabled={locked} onChange={e => setSelected(e.target.value)}>
          {!items.length && <option value="">{loading ? "读取候选中…" : "没有可用候选"}</option>}
          {items.map(w => <option key={w.doc.id} value={w.doc.id}>{w.doc.id} · {w.doc.name} · 修订 {w.revision}</option>)}
        </select>
        <SamplePartSelect sampleSet={site.sampleSet} nextPart={site.nextPart} value={partId} onChange={setPartId} disabled={locked} label="调试样本件" />
        <button className="btn" disabled={locked} onClick={() => setRefresh(v => v + 1)}>刷新候选</button>
        <button className="btn primary" disabled={locked || !candidate || !!candidate.pending} onClick={() => void run()}>{starting ? "准备试运行…" : "候选试运行一圈"}</button>
      </div>
      {error && <p className="c-ng" role="alert">{error}</p>}
    </details>
  </section>;
}
