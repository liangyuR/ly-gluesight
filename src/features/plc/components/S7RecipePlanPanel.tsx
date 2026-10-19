import { useEffect, useRef, useState } from "react";
import { Accordion, Alert, Button, Card, Heading, Select, Stack, Table, Text, type TableColumn } from "qing-steel";
import { plcApi } from "../api";
import type { PlcRecipeChoice, PlcRecipePlan } from "../types";

type Shot = PlcRecipePlan["shots"][number];
const shotColumns: TableColumn<Shot>[] = [
  { key: "shotId", header: "拍照点", render: shot => <span className="mono">{shot.shotId}</span> },
  { key: "poseId", header: "Pose", render: shot => <span className="mono">{shot.poseId}</span> },
  { key: "cameraIds", header: "相机设备 ID", render: shot => <span className="mono">{shot.cameraIds.join("、")}</span> },
];
const deviceColumns: TableColumn<PlcRecipePlan>[] = [
  { key: "deviceIds", header: "相机设备 ID", render: plan => <span className="mono">{plan.deviceIds.join("、")}</span> },
  { key: "deviceShots", header: "每台计划触发数", render: plan => plan.deviceShots },
];

export default function S7RecipePlanPanel() {
  const [recipes, setRecipes] = useState<PlcRecipeChoice[] | null>(null);
  const [selected, setSelected] = useState("");
  const [plan, setPlan] = useState<PlcRecipePlan | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copied, setCopied] = useState(false);
  const mounted = useRef(true);
  const serial = useRef(0);
  const copyPending = useRef(false);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; serial.current++; }; }, []);
  useEffect(() => { if (window.location.hash.includes("panel=s7-plan")) root.current?.scrollIntoView?.({ block: "start" }); }, []);

  const readPlan = async (recipeId: string) => {
    const request = ++serial.current;
    setSelected(recipeId); setPlan(null); setError(""); setCopied(false);
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
    const request = ++serial.current;
    setLoading(true); setError(""); setPlan(null); setCopied(false);
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

  const copy = async () => {
    if (!plan || copyPending.current) return;
    const request = serial.current;
    copyPending.current = true; setCopying(true); setError("");
    try {
      await navigator.clipboard.writeText(JSON.stringify(plan, null, 2));
      if (mounted.current && request === serial.current) setCopied(true);
    } catch (reason) {
      if (mounted.current && request === serial.current) setError(`复制失败：${String(reason)}`);
    } finally { copyPending.current = false; if (mounted.current) setCopying(false); }
  };

  return <div id="s7-plan" ref={root}>
    <Card title={<Heading level={3}>配方握手计划 · 只读</Heading>} actions={<>
      {plan && <Button disabled={copying} onClick={() => void copy()}>{copied ? "已复制 JSON" : "复制计划 JSON"}</Button>}
      <Button disabled={loading} onClick={() => void load()}>{recipes ? "刷新配方计划" : "读取配方计划"}</Button>
    </>}>
      <Stack direction="column" align="stretch" gap={12}>
        <Text as="p" variant="label" muted>读取已发布配方，供 PLC 程序核对协议版本、计划版本和设备触发数。读取和复制不会更改 PLC；候选试运行的临时计划不在此处显示；离线发布版本不代表真机可用。</Text>
        {recipes && <Select label="已发布配方" value={selected} onValueChange={value => void readPlan(value)}
          options={[{ value: "", label: "请选择已发布配方" }, ...recipes.map(recipe => ({ value: recipe.id, label: `${recipe.name} · ${recipe.id} · v${recipe.version}` }))]} />}
        {recipes?.length === 0 && <Text as="p" variant="label" muted>暂无可读取的已发布配方</Text>}
        {loading && <p role="status">正在读取配方计划…</p>}
        {error && <Alert tone="ng">{error}</Alert>}
        {plan && <>
          <Text as="p" variant="label">协议 {plan.protocolVersion} · 计划版本 {plan.planVersion} · 共 {plan.shotCount} 个拍照点</Text>
          <Table<PlcRecipePlan> caption="触发线相机" rows={[plan]} columns={deviceColumns} rowKey={() => "devices"} />
          <Table<Shot> caption="拍照点计划" rows={plan.shots} columns={shotColumns} rowKey={shot => shot.shotId} />
          <Text as="p" variant="label" muted>PLC 每次触发同时触发上表全部相机，每台各出一帧；同一 Pose 的各相机拍照点算一次触发，拼接相机的三个视角也不分别计数，计划触发数等于触发次数。</Text>
          <Accordion items={[{ value: "json", title: "查看完整计划 JSON", content: <pre className="mono" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", fontSize: 12 }}>{JSON.stringify(plan, null, 2)}</pre> }]} />
        </>}
      </Stack>
    </Card>
  </div>;
}
