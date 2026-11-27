import { Fragment, useState } from "react";
import { Alert, Button, Dialog, List, ListItem, NumberField, SegmentedControl, Stack, StatusBadge, Table, Text, type TableColumn } from "qing-steel";
import type { Verdict } from "../../cycle/types";
import { formatTime, verdictGroups, verdictLabel, verdictTone } from "../meta";
import { useRejudgeTask, type RejudgeOptions, type RejudgeScope } from "../rejudgeTask";
import type { KindOverride, Overrides } from "../types";

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
export default function RejudgeDialog({ scope, originalScope, onUseCurrentScope, onOpenRecord, onClose }: {
  scope: RejudgeScope;
  originalScope: boolean;
  onUseCurrentScope?: () => void;
  onOpenRecord: (id: number) => void;
  onClose: () => void;
}) {
  const { task, run } = useRejudgeTask();
  const scopedTask = task?.scope === scope ? task : null;
  const [options, setOptions] = useState<RejudgeOptions>(() => scopedTask?.options
    ?? { useCurrentRecipe: false, overrides: { straight: {}, corner: {} } });
  const { useCurrentRecipe, overrides } = options;
  const matchingTask = scopedTask?.options === options ? scopedTask : null;
  const running = task?.status === "running";
  const result = matchingTask?.status === "completed" ? matchingTask.result : null;
  const error = matchingTask?.status === "failed" ? matchingTask.error : "";
  const invalid = Object.values(overrides).some(value => typeof value === "number" && !Number.isFinite(value)) ||
    [overrides.straight, overrides.corner].some(value =>
      Object.values(value).some(v => v !== undefined && (!Number.isFinite(v) || v < 0))) ||
    (overrides.maxGapLen !== undefined && overrides.maxGapLen < 0);
  const edit = (next: Overrides) => setOptions(current => ({ ...current, overrides: next }));
  const runLabel = originalScope ? "按原筛选重判"
    : scopedTask ? "再次重判" : `重判当前筛选（${scope.total} 件）`;

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
          <Button onClick={onClose}>{running ? "收起" : "关闭"}</Button>
          {!running && onUseCurrentScope && <Button onClick={onUseCurrentScope}>使用当前列表</Button>}
          <Button variant="filled" onClick={() => void run(scope, options)} disabled={running || invalid || scope.total <= 0}>
            {running ? "重判中…" : runLabel}
          </Button>
        </>
      }
    >
      <Stack direction="column" align="stretch" gap={14}>
        {running && <div role="status">正在重判。收起后可从历史页查看进度和结果。</div>}
        {originalScope && <Text muted>此任务使用原筛选范围，当前历史列表的筛选已变化。</Text>}
        <SegmentedControl label="重判使用的配方" options={recipeOptions} disabled={running} value={useCurrentRecipe ? "current" : "recorded"} onValueChange={(value) => setOptions(current => ({ ...current, useCurrentRecipe: value === "current" }))} />
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
              {result.skipReasons.map(item => <ListItem key={item.id} title={<span className="sn">SN {item.sn}</span>} description={item.reason} onClick={() => onOpenRecord(item.id)} />)}
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
                    onClick={() => onOpenRecord(c.id)}
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
