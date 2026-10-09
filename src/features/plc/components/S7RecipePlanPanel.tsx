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
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; serial.current++; }; }, []);

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

  return <div className="panel">
    <div className="panel-toolbar"><h3 className="panel-title">配方握手计划 · 只读</h3><div className="row">
      {plan && <button className="btn" disabled={copying} onClick={() => void copy()}>{copied ? "已复制 JSON" : "复制计划 JSON"}</button>}
      <button className="btn" disabled={loading} onClick={() => void load()}>{recipes ? "刷新配方计划" : "读取配方计划"}</button>
    </div></div>
    <p className="muted hint top">读取已保存的生产配方，供 PLC 程序核对协议版本、计划摘要和各相机点数。读取和复制不会更改 PLC；候选草稿不在此处生效。</p>
    {recipes && <label className="field"><span>生产配方</span><select className="input" value={selected} onChange={event => void readPlan(event.target.value)}>
      <option value="">请选择生产配方</option>{recipes.map(recipe => <option key={recipe.id} value={recipe.id}>{recipe.name} · {recipe.id} · v{recipe.version}</option>)}
    </select></label>}
    {recipes?.length === 0 && <p className="muted hint">暂无可读取的生产配方</p>}
    {loading && <p role="status">正在读取配方计划…</p>}
    {error && <p role="alert" className="notice error">{error}</p>}
    {plan && <>
      <p className="hint">协议 {plan.protocolVersion} · 计划版本 {plan.planVersion} · 摘要 <span className="mono">{plan.planHash}</span> · 共 {plan.shotCount} 个拍照点</p>
      <div className="table-wrap"><table className="table"><thead><tr><th>PLC 相机槽位</th><th>相机编号</th><th>计划点数</th></tr></thead><tbody>
        {plan.cameraSlots.map((camera, index) => <tr key={index}><td>相机 {index + 1}</td><td>{camera || "未配置"}</td><td>{plan.cameraShots[index]}</td></tr>)}
      </tbody></table></div>
      <div className="table-wrap"><table className="table" aria-label="拍照点计划"><thead><tr><th>拍照点</th><th>Pose</th><th>相机编号</th><th>中心（mm）</th></tr></thead><tbody>
        {plan.shots.map(shot => <tr key={shot.shotId}><td className="mono">{shot.shotId}</td><td className="mono">{shot.poseId}</td><td className="mono">{shot.cameraId}</td><td className="mono">{shot.center.map(v => v.toFixed(1)).join(", ")}</td></tr>)}
      </tbody></table></div>
      <p className="muted hint">这里只展示当前配方的真实绑定；同一 Pose 可以触发几台相机。多相机配方可以导出计划，帧归属接入前不能布防；三个槽位不代表三相机飞拍已完成。</p>
      <details><summary>查看完整计划 JSON</summary><pre className="mono" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", fontSize: 12 }}>{JSON.stringify(plan, null, 2)}</pre></details>
    </>}
  </div>;
}
