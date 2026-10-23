import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { AlertTriangle, ArrowRight, Check, LoaderCircle } from "lucide-react";
import { Button, Surface } from "qing-steel";
import { RecipeEditor, changedFields } from "../recipe";
import { recipeApi } from "../cycle/api";
import type { RecipeDoc } from "../cycle/types";
import { expectedText } from "../plan/vis";
import { cameraTeachingTasks } from "./cameraTasks";
import { useWorkspace } from "./context";
import { Badge, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { targetPathV2 } from "./progress";
import { stageOfV2 } from "./stages";

const saveText = { saved: "已保存", pending: "等待保存", saving: "保存中…", failed: "保存失败" } as const;

export default function GeometryPage() {
  const { data, doc, setDoc, previewError, busy, saveState, retrySave } = useWorkspace();
  const [production, setProduction] = useState<RecipeDoc | null>(null);
  const productionVersion = data?.productionVersion ?? null;
  useEffect(() => {
    let alive = true;
    setProduction(null);
    if (!doc?.id || productionVersion == null) return;
    recipeApi.doc(doc.id).then(d => { if (alive) setProduction(d); }).catch(() => {});
    return () => { alive = false; };
  }, [doc?.id, productionVersion]);
  if (!data || !doc || !data.plan) return <WorkspaceEmpty />;
  const plan = data.plan;
  const changed = changedFields(doc, production).size;
  const next = plan.progress.next && { label: plan.progress.next.label, stage: stageOfV2(plan.progress.next.step)?.label, path: targetPathV2(plan.progress.next.target) };
  const taught = plan ? cameraTeachingTasks(plan).filter(task => task.status === "done").length : 0;
  const straight = plan?.segments.filter(segment => segment.kind === "straight").length ?? 0;
  return <div className="wp-page"><WorkspaceBar />
    <div className="wp-columns wp-defaults">
      <div className="wp-stack">
        <Panel title="配方默认参数" detail="走廊与新建的段没单独设置时使用这里的值">
          <fieldset disabled={busy || !!data.workspace.pending} className="wp-plain-fieldset">
            <RecipeEditor key={doc.id + ":" + data.workspace.revision} initial={doc} production={production} onDraftChange={setDoc} disabled={busy || !!data.workspace.pending} />
          </fieldset>
        </Panel>
      </div>
      <div className="wp-stack">
        <Surface role="region" aria-label="配方摘要" className="wp-panel wp-defaults-card">
          <h3>配方摘要</h3>
          <dl>
            <dt>段</dt><dd>{plan.segments.length}<small> · 直线 {straight} · 弯道 {plan.segments.length - straight}</small></dd>
            <dt>相机</dt><dd>{plan.cameras.length} 台<small> · 高电平启停</small></dd>
            <dt>预期总帧数</dt><dd>{expectedText(plan.expectedTotal, plan.toleranceTotal)}</dd>
            <dt>走廊已示教</dt><dd>{taught} / {plan.cameras.length}</dd>
          </dl>
        </Surface>
        {next && <section className="wp-defaults-next" aria-label="下一步">
          <small>下一步 · {next.stage ?? "配方进度"}</small>
          <strong>{next.label}</strong>
          <Link to={next.path}>前往<ArrowRight size={14} aria-hidden="true" /></Link>
        </section>}
      </div>
    </div>
    <div className="wp-defaults-savebar" role="status" aria-label="候选保存状态">
      {productionVersion != null ? <Badge tone={changed ? "info" : "neutral"}>{changed ? `${changed} 项与生产 v${productionVersion} 不同` : `与生产 v${productionVersion} 一致`}</Badge> : <Badge tone="neutral">尚未发布</Badge>}
      <span className="muted">修改自动保存为候选配置，不影响在线生产配方</span>
      <span className="spacer" />
      {previewError ? <span className="wp-defaults-save warn"><AlertTriangle size={14} aria-hidden="true" />参数无效：{previewError}</span> : <span className={"wp-defaults-save " + saveState}>{saveState === "saved" ? <Check size={14} aria-hidden="true" /> : saveState === "failed" ? <AlertTriangle size={14} aria-hidden="true" /> : <LoaderCircle size={14} aria-hidden="true" />}{saveText[saveState]}</span>}
      {saveState === "failed" && <Button size="sm" onClick={retrySave}>重试保存</Button>}
    </div>
  </div>;
}
