import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Alert, Button, Card, Heading, Select, Stack, Table, Text, type TableColumn } from "qing-steel";
import { plcApi } from "../api";
import type { PlcRecipeChoice, PlcRecipePlan } from "../types";

type Param = { tag: string; value: number; note: string };
const paramColumns: TableColumn<Param>[] = [
  { key: "tag", header: "PLC 标签", render: param => <span className="mono">{param.tag}</span> },
  { key: "value", header: "值", render: param => <span className="mono">{param.value}</span> },
  { key: "note", header: "说明", render: param => param.note },
];
type Shot = PlcRecipePlan["shots"][number] & { order: number };
const shotColumns: TableColumn<Shot>[] = [
  { key: "order", header: "顺序", render: shot => shot.order },
  { key: "shotId", header: "拍照点", render: shot => <span className="mono">{shot.shotId}</span> },
  { key: "poseId", header: "Pose", render: shot => <span className="mono">{shot.poseId}</span> },
];

function scrollToPlan(panel: HTMLDivElement | null, behavior: ScrollBehavior = "smooth") {
  const scroller = panel?.closest<HTMLElement>(".main-body");
  if (!panel || !scroller) return;
  const toolbar = panel.closest(".plc")?.querySelector(".plc-actionbar");
  const inset = (toolbar?.getBoundingClientRect().height ?? 0) + 16;
  // 只移动内容区；scrollIntoView 会连同 overflow:hidden 的外层应用一起滚动。
  scroller.scrollTo({ top: scroller.scrollTop + panel.getBoundingClientRect().top - scroller.getBoundingClientRect().top - inset, behavior });
}

export default function S7RecipePlanPanel({ actionsTarget }: { actionsTarget?: HTMLElement | null }) {
  const [recipes, setRecipes] = useState<PlcRecipeChoice[] | null>(null);
  const [selected, setSelected] = useState("");
  const [plan, setPlan] = useState<PlcRecipePlan | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const mounted = useRef(true);
  const serial = useRef(0);
  const scrollPending = useRef(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; serial.current++; }; }, []);
  useEffect(() => {
    if (!window.location.hash.includes("panel=s7-plan")) return;
    const frame = requestAnimationFrame(() => scrollToPlan(root.current, "auto"));
    return () => cancelAnimationFrame(frame);
  }, []);
  useEffect(() => {
    if (loading || !scrollPending.current) return;
    scrollPending.current = false;
    scrollToPlan(root.current);
  }, [loading]);

  const readPlan = async (recipeId: string) => {
    const request = ++serial.current;
    setSelected(recipeId); setPlan(null); setError("");
    setLoading(!!recipeId);
    if (!recipeId) return;
    try {
      const result = await plcApi.recipePlan(recipeId);
      if (mounted.current && serial.current === request) setPlan(result);
    } catch (reason) {
      if (mounted.current && serial.current === request) setError(String(reason));
    } finally { if (mounted.current && serial.current === request) setLoading(false); }
  };

  const load = async () => {
    if (loading) return;
    scrollPending.current = true;
    const request = ++serial.current;
    setLoading(true); setError(""); setPlan(null);
    try {
      const [choices, state] = await Promise.all([plcApi.recipeChoices(), plcApi.getOperationState().catch(() => null)]);
      if (!mounted.current || serial.current !== request) return;
      setRecipes(choices);
      const next = choices.find(recipe => recipe.id === selected)?.id
        ?? choices.find(recipe => recipe.id === state?.activeRecipeId)?.id ?? "";
      setLoading(false);
      await readPlan(next);
    } catch (reason) {
      if (mounted.current && serial.current === request) { setError(String(reason)); setLoading(false); }
    }
  };

  const recipe = recipes?.find(choice => choice.id === plan?.recipeId);
  const params: Param[] = plan && recipe ? [
    { tag: "productCode", value: recipe.productCode, note: "产品代码，与配方不一致时返回 95" },
    { tag: "planVersion", value: plan.planVersion, note: "计划版本，拍照顺序或 Pose 变化后会换新号；不一致时返回 100" },
    { tag: "shotCount", value: plan.shotCount, note: "触发次数，deviceShots 填同一值；不一致时返回 94" },
  ] : [];
  const actions = <Button disabled={loading} onClick={() => void load()}>{recipes ? "刷新对接参数" : "读取对接参数"}</Button>;

  return <div id="s7-plan" ref={root}>
    {actionsTarget && createPortal(actions, actionsTarget)}
    <Card title={<Heading level={3}>PLC 对接参数 · 只读</Heading>} actions={actionsTarget ? undefined : actions}>
      <Stack direction="column" align="stretch" gap={12}>
        <Text as="p" variant="label" muted>选择已发布配方，把下列参数配到 PLC 程序的开始请求里；PC 接件时逐项核对，不一致则拒绝布防。读取不会更改 PLC；候选试运行的临时计划不在此处显示；离线发布版本不代表真机可用。</Text>
        {recipes && <Select label="已发布配方" value={selected} onValueChange={value => void readPlan(value)}
          options={[{ value: "", label: "请选择已发布配方" }, ...recipes.map(recipe => ({ value: recipe.id, label: `${recipe.name} · ${recipe.id} · v${recipe.version}` }))]} />}
        {recipes?.length === 0 && <Text as="p" variant="label" muted>暂无可读取的已发布配方</Text>}
        {loading && <p role="status">正在读取对接参数…</p>}
        {error && <Alert tone="ng">{error}</Alert>}
        {plan && <>
          <Table<Param> caption="开始请求参数" rows={params} columns={paramColumns} rowKey={param => param.tag} />
          <Table<Shot> caption="拍照顺序与 Pose" rows={plan.shots.map((shot, index) => ({ ...shot, order: index + 1 }))} columns={shotColumns} rowKey={shot => shot.shotId} />
          <Text as="p" variant="label" muted>机器人按上表顺序经过各拍照点，每个拍照点发一次触发；同一 Pose 可被多个拍照点引用，三目设备的三个视角不分别计数。</Text>
        </>}
      </Stack>
    </Card>
  </div>;
}
