import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import Modal from "../../plc/components/Modal";
import type { Verdict } from "../../cycle/types";
import { historyApi } from "../api";
import { formatTime, verdictClass, verdictGroups, verdictLabel } from "../meta";
import type { HistoryQuery, KindOverride, Overrides, RejudgeResult } from "../types";

const order: Verdict[] = verdictGroups.flatMap((g) => g.verdicts);
const kindFields: [keyof KindOverride, string][] = [
  ["tolUpper", "上公差"],
  ["tolLower", "下公差"],
  ["maxExcursionLen", "连续超差允许长度"],
  ["absMin", "绝对限下"],
  ["absMax", "绝对限上"],
];

const isNg = (v: Verdict) => v.startsWith("NG");
const isOk = (v: Verdict) => v.startsWith("OK");

function numOrUndef(s: string) {
  return s.trim() === "" ? undefined : Number(s);
}

/** 用存储的测量表批量重判：可选当前配方与试算参数，输出判定变化矩阵。 */
export default function RejudgeDialog({ query, total, onClose }: { query: HistoryQuery; total: number; onClose: () => void }) {
  const navigate = useNavigate();
  const [useCurrent, setUseCurrent] = useState(false);
  const [overrides, setOverrides] = useState<Overrides>({ line: {}, corner: {}, width: {} });
  const [result, setResult] = useState<RejudgeResult | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const scope=JSON.stringify(query);
  const current=useRef({scope,alive:true});
  current.current.scope=scope;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  useEffect(()=>{setResult(null);setError("");},[scope]);
  const invalid=Object.values(overrides).some(value=>typeof value==="number"&&!Number.isFinite(value))||
    [overrides.line,overrides.corner,overrides.width].some(value=>value&&(
      Object.values(value).some(v=>v!==undefined&&!Number.isFinite(v))||
      [value.tolUpper,value.tolLower,value.maxExcursionLen].some(v=>v!==undefined&&v<0)||
      (value.absMin!==undefined&&value.absMax!==undefined&&value.absMin>value.absMax)))||
    (overrides.maxGapLen!==undefined&&overrides.maxGapLen<0)||
    (overrides.filterWindow!==undefined&&(!Number.isInteger(overrides.filterWindow)||overrides.filterWindow<1||overrides.filterWindow>31||overrides.filterWindow%2===0));
  const edit=(next:Overrides)=>{setOverrides(next);setResult(null);setError("");};

  const run = async () => {
    if(pending.current||invalid||total<=0)return;
    pending.current=true;
    setRunning(true);
    setError("");
    const valid=()=>current.current.alive&&current.current.scope===scope;
    try {
      const next=await historyApi.rejudge({ query, ids: [], useCurrentRecipe: useCurrent, overrides });
      if(valid())setResult(next);
    } catch (e) {
      if(valid())setError(String(e));
    } finally {
      pending.current=false;if(current.current.alive)setRunning(false);
    }
  };

  const kind = (k: "line" | "corner" | "width", f: keyof KindOverride) => (
    <input
      id={`rj-${k}-${f}`}
      className="input mono"
      type="number"
      step={0.05}
      placeholder="不变"
      aria-label={`${k==="line"?"直边":k==="corner"?"R 角":"胶宽"}${kindFields.find(([key])=>key===f)?.[1]}`}
      disabled={running}
      value={overrides[k]?.[f] ?? ""}
      onChange={(e) => edit({ ...overrides, [k]: { ...overrides[k], [f]: numOrUndef(e.target.value) } })}
    />
  );

  const cell = (from: Verdict, to: Verdict) => result?.matrix.find((m) => m.from === from && m.to === to)?.count ?? 0;
  const released = result ? result.matrix.filter((m) => isNg(m.from) && isOk(m.to)).reduce((a, m) => a + m.count, 0) : 0;

  return (
    <Modal
      title="批量重判"
      width={820}
      onClose={onClose}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" onClick={onClose}>关闭</button>
          <button className="btn primary" onClick={run} disabled={running||invalid||total<=0}>{running ? "重判中…" : `重判当前筛选（${total} 件）`}</button>
        </>
      }
    >
      <div className="rj">
        <p className="muted">用已存储的测量表重新判定，不需要重新拍照；ERR 件和缺少测量数据的记录会跳过。试算参数只用于本次重判，不会保存到配方。</p>
        <div className="segmented">
          <button className={!useCurrent ? "active" : ""} disabled={running} onClick={() => {setUseCurrent(false);setResult(null);}}>记录当时的配方版本</button>
          <button className={useCurrent ? "active" : ""} disabled={running} onClick={() => {setUseCurrent(true);setResult(null);}}>当前配方</button>
        </div>
        <div className="rj-params">
          <span />
          {kindFields.map(([, label]) => (
            <span key={label} className="muted">{label}（mm）</span>
          ))}
          <span>直边</span>
          {kindFields.map(([f]) => <div key={f}>{kind("line", f)}</div>)}
          <span>R 角</span>
          {kindFields.map(([f]) => <div key={f}>{kind("corner", f)}</div>)}
          <span title="只作用于配置了胶宽判定的段">胶宽</span>
          {kindFields.map(([f]) => <div key={f}>{kind("width", f)}</div>)}
        </div>
        <div className="row">
          <label className="field">
            <span>断胶允许长度（mm）</span>
            <input id="rj-gap" className="input mono" aria-label="断胶允许长度（mm）" disabled={running} type="number" min={0} step={0.1} placeholder="不变" value={overrides.maxGapLen ?? ""} onChange={(e) => edit({ ...overrides, maxGapLen: numOrUndef(e.target.value) })} />
          </label>
          <label className="field">
            <span>滤波窗口（奇数）</span>
            <input id="rj-filter" className="input mono" aria-label="滤波窗口（奇数）" disabled={running} type="number" step={2} min={1} max={31} placeholder="不变" value={overrides.filterWindow ?? ""} onChange={(e) => edit({ ...overrides, filterWindow: numOrUndef(e.target.value) })} />
          </label>
        </div>

        {invalid&&<p className="form-error" role="alert">试算参数需为有限数，公差与允许长度不能为负，绝对限下需不大于上限；滤波窗口需为 1–31 的奇数。</p>}

        {result && (
          <>
            <div className="rj-summary">
              重判 <b>{result.total}</b> 件，跳过 <b>{result.skipped}</b> 件{result.limitHit && "（超过 5000 件，只取最近 5000 件）"}
              {released > 0 && <span className="rj-alert">NG→OK {released} 件：放宽后可能放过真实缺陷，请逐件复核</span>}
            </div>
            <div className="table-wrap">
              <table className="table rj-matrix">
                <thead>
                  <tr>
                    <th>原结果 ＼ 新结果</th>
                    {order.map((v) => <th key={v}>{verdictLabel[v]}</th>)}
                  </tr>
                </thead>
                <tbody>
                  {order.map((from) => (
                    <tr key={from}>
                      <td><span className={`vt ${verdictClass(from)}`}>{verdictLabel[from]}</span></td>
                      {order.map((to) => {
                        const n = cell(from, to);
                        const cls = n === 0 ? "zero" : from === to ? "same" : isNg(from) && isOk(to) ? "release" : "changed";
                        return <td key={to} className={`mono ${cls}`}>{n}</td>;
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {result.changes.length > 0 && (
              <div className="rj-changes">
                {result.changes.map((c) => (
                  <button key={c.id} className="rj-change" onClick={() => navigate(`/history/${c.id}`)}>
                    <span className="mono">{formatTime(c.ts)}</span>
                    <span className="mono">SN {c.sn}</span>
                    <span className={`vt ${verdictClass(c.from)}`}>{verdictLabel[c.from]}</span>→
                    <span className={`vt ${verdictClass(c.to)}`}>{verdictLabel[c.to]}</span>
                    <span className="muted ellipsis">{c.reason}</span>
                  </button>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
