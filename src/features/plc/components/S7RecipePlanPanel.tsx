import { useEffect, useRef, useState } from "react";
import { plcApi } from "../api";
import type { PlcRecipeChoice, PlcRecipePlan } from "../types";

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

  return <div className="panel" id="s7-plan" ref={root}>
    <div className="panel-toolbar"><h3 className="panel-title">配方握手计划 · 只读</h3><div className="row">
      {plan && <button className="btn" disabled={copying} onClick={() => void copy()}>{copied ? "已复制 JSON" : "复制计划 JSON"}</button>}
      <button className="btn" disabled={loading} onClick={() => void load()}>{recipes ? "刷新配方计划" : "读取配方计划"}</button>
    </div></div>
    <p className="muted hint top">读取已保存的生产配方，供 PLC 程序核对协议版本、计划版本和设备触发数。读取和复制不会更改 PLC；候选草稿不在此处生效。</p>
    {recipes && <label className="field"><span>生产配方</span><select className="input" value={selected} onChange={event => void readPlan(event.target.value)}>
      <option value="">请选择生产配方</option>{recipes.map(recipe => <option key={recipe.id} value={recipe.id}>{recipe.name} · {recipe.id} · v{recipe.version}</option>)}
    </select></label>}
    {recipes?.length === 0 && <p className="muted hint">暂无可读取的生产配方</p>}
    {loading && <p role="status">正在读取配方计划…</p>}
    {error && <p role="alert" className="notice error">{error}</p>}
    {plan && <>
      <p className="hint">协议 {plan.protocolVersion} · 计划版本 {plan.planVersion} · 共 {plan.shotCount} 个拍照点</p>
      <div className="table-wrap"><table className="table" aria-label="单设备计划"><thead><tr><th>相机设备 ID</th><th>计划触发数</th></tr></thead><tbody>
        <tr><td className="mono">{plan.deviceId}</td><td>{plan.deviceShots}</td></tr>
      </tbody></table></div>
      <div className="table-wrap"><table className="table" aria-label="拍照点计划"><thead><tr><th>拍照点</th><th>Pose</th><th>相机设备 ID</th></tr></thead><tbody>
        {plan.shots.map(shot => <tr key={shot.shotId}><td className="mono">{shot.shotId}</td><td className="mono">{shot.poseId}</td><td className="mono">{shot.cameraId}</td></tr>)}
      </tbody></table></div>
      <p className="muted hint">每次触发唯一相机设备，SDK 返回一幅包含图 1 / 图 2 / 图 3 的拼接图。三个视角不分别计数，计划触发数等于拍照点数。</p>
      <details><summary>查看完整计划 JSON</summary><pre className="mono" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", fontSize: 12 }}>{JSON.stringify(plan, null, 2)}</pre></details>
    </>}
  </div>;
}
