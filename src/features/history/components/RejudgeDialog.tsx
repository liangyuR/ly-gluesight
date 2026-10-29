import { Fragment, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Alert, Button, Dialog, List, ListItem, NumberField, SegmentedControl, Stack, StatusBadge, Table, Text, type TableColumn } from "qing-steel";
import type { Verdict } from "../../cycle/types";
import { historyApi } from "../api";
import { formatTime, verdictGroups, verdictLabel, verdictTone } from "../meta";
import type { HistoryQuery, KindOverride, Overrides, RejudgeResult } from "../types";
import { errorText } from "../../../lib/errors";

const order: Verdict[] = verdictGroups.flatMap((g) => g.verdicts);
const kindFields: [keyof KindOverride, string][] = [
  ["tolUpper", "上公差"],
  ["tolLower", "下公差"],
  ["maxExcursionLen", "帧内累计超差允许长度"],
];
const recipeOptions = [
  { value: "recorded", label: "记录当时的配方版本" },
  { value: "current", label: "当前配方" },
];

const isNg = (v: Verdict) => v.startsWith("NG");
const isOk = (v: Verdict) => v.startsWith("OK");
const orDefault = (value: number | "") => (value === "" ? undefined : value);

/** 用存储的测量表批量重判：可选当前配方与试算参数，输出判定变化矩阵。 */
export default function RejudgeDialog({ query, total, onClose }: { query: HistoryQuery; total: number; onClose: () => void }) {
  const navigate = useNavigate();
  const [useCurrent, setUseCurrent] = useState(false);
  const [overrides, setOverrides] = useState<Overrides>({ straight: {}, corner: {} });
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
    [overrides.straight,overrides.corner].some(value=>
      Object.values(value).some(v=>v!==undefined&&(!Number.isFinite(v)||v<0)))||
    (overrides.maxGapLen!==undefined&&overrides.maxGapLen<0);
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
      if(valid())setError(errorText(e));
    } finally {
      pending.current=false;if(current.current.alive)setRunning(false);
    }
  };

  const kindName = { straight: "直线段胶宽", corner: "弯道段胶宽" };
  const kind = (k: keyof typeof kindName, f: keyof KindOverride) => (
    <NumberField
      id={`rj-${k}-${f}`}
      step={0.05}
      placeholder="不变"
      aria-label={`${kindName[k]}${kindFields.find(([key])=>key===f)?.[1]}`}
      disabled={running}
      value={overrides[k]?.[f] ?? ""}
      onValueChange={(value) => edit({ ...overrides, [k]: { ...overrides[k], [f]: orDefault(value) } })}
    />
  );

  const cell = (from: Verdict, to: Verdict) => result?.matrix.find((m) => m.from === from && m.to === to)?.count ?? 0;
  const released = result ? result.matrix.filter((m) => isNg(m.from) && isOk(m.to)).reduce((a, m) => a + m.count, 0) : 0;
  const matrixColumns: TableColumn<Verdict>[] = [
    { key: "from", header: "原结果 ＼ 新结果", render: (from) => <StatusBadge tone={verdictTone(from)}>{verdictLabel[from]}</StatusBadge> },
    ...order.map((to): TableColumn<Verdict> => ({
      key: to,
      header: verdictLabel[to],
      align: "center",
      render: (from) => {
        const n = cell(from, to);
        const cls = n === 0 ? "zero" : from === to ? "same" : isNg(from) && isOk(to) ? "release" : "changed";
        return <span className={`rj-cell mono ${cls}`}>{n}</span>;
      },
    })),
  ];

  return (
    <Dialog
      open
      onOpenChange={(open) => { if (!open) onClose(); }}
      title="批量重判"
      className="rj-dialog"
      description="用已存储的测量表重新判定，不需要重新拍照；ERR 件和缺少测量数据的记录会跳过。试算参数只用于本次重判，不会保存到配方。"
      footer={
        <>
          <Button onClick={onClose}>关闭</Button>
          <Button variant="filled" onClick={run} disabled={running||invalid||total<=0}>{running ? "重判中…" : `重判当前筛选（${total} 件）`}</Button>
        </>
      }
    >
      <Stack direction="column" align="stretch" gap={14}>
        <SegmentedControl label="重判使用的配方" options={recipeOptions} disabled={running} value={useCurrent ? "current" : "recorded"} onValueChange={(value) => { setUseCurrent(value === "current"); setResult(null); }} />
        <div className="rj-params">
          <span />
          {kindFields.map(([, label]) => (
            <Text key={label} variant="label" muted>{label}（mm）</Text>
          ))}
          {(["straight", "corner"] as const).map(k => <Fragment key={k}>
            <span title={`只作用于${k === "straight" ? "直线" : "弯道"}区域的每帧胶宽`}>{k === "straight" ? "直线段" : "弯道段"}</span>
            {kindFields.map(([f]) => <div key={f}>{kind(k, f)}</div>)}
          </Fragment>)}
        </div>
        <Text variant="label" muted>胶宽允差按每帧独立判，未分段区域取直线、弯道中较宽的一档。</Text>
        <div className="rj-pair">
          <NumberField id="rj-gap" label="允许断胶长度（mm，每帧）" aria-label="允许断胶长度（mm）" disabled={running} min={0} step={0.1} placeholder="不变" value={overrides.maxGapLen ?? ""} onValueChange={(value) => edit({ ...overrides, maxGapLen: orDefault(value) })} />
        </div>

        {invalid&&<Alert tone="ng">试算参数需为有限数，公差与允许长度不能为负。</Alert>}
        {error && <Alert tone="ng">{error}</Alert>}

        {result && (
          <>
            <Stack gap={14}>
              <span>重判 <b>{result.total}</b> 件，跳过 <b>{result.skipped}</b> 件{result.limitHit && "（超过 5000 件，只取最近 5000 件）"}</span>
              {released > 0 && <StatusBadge tone="ng">NG→OK {released} 件：放宽后可能放过真实缺陷，请逐件复核</StatusBadge>}
            </Stack>
            {!!result.skipReasons.length && <List className="rj-changes" aria-label="跳过原因">
              {result.skipReasons.map(item => <ListItem key={item.id} title={<span className="sn">SN {item.sn}</span>} description={item.reason} onClick={() => navigate(`/history/${item.id}`)} />)}
            </List>}
            <Table<Verdict> caption="判定变化矩阵" rows={order} columns={matrixColumns} rowKey={(v) => v} />
            {result.changes.length > 0 && (
              <List className="rj-changes" aria-label="判定变化记录">
                {result.changes.map((c) => (
                  <ListItem
                    key={c.id}
                    leading={<span className="mono">{formatTime(c.ts)}</span>}
                    title={<>
                      <span className="sn">SN {c.sn}</span>{" "}
                      <StatusBadge tone={verdictTone(c.from)}>{verdictLabel[c.from]}</StatusBadge>→<StatusBadge tone={verdictTone(c.to)}>{verdictLabel[c.to]}</StatusBadge>
                    </>}
                    description={c.reason}
                    onClick={() => navigate(`/history/${c.id}`)}
                  />
                ))}
              </List>
            )}
          </>
        )}
      </Stack>
    </Dialog>
  );
}
