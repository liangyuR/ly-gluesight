import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { AlertTriangle, ArrowRight, Check, LoaderCircle } from "lucide-react";
import { RecipeEditor, changedFields } from "../recipe";
import { recipeApi } from "../cycle/api";
import { segmentLength, shotTaught } from "../cycle/vis";
import type { RecipeDoc } from "../cycle/types";
import { useWorkspace } from "./context";
import { Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import { targetPath } from "./progress";
import { stageOf } from "./stages";

const saveText = { saved: "已保存", pending: "等待保存", saving: "保存中…", failed: "保存失败" } as const;

/** 配方默认检测与判定参数；拍照点由整圈采集生成，中线在单帧示教里点出。 */
export default function GeometryPage() {
  const { data, doc, setDoc, preview, previewError, busy, saveState, retrySave } = useWorkspace();
  const [production, setProduction] = useState<RecipeDoc | null>(null);
  const productionVersion = data?.productionVersion ?? null;
  useEffect(() => {
    let alive = true;
    setProduction(null);
    if (!doc?.id || productionVersion == null) return;
    recipeApi.doc(doc.id).then(d => { if (alive) setProduction(d); }).catch(() => {});
    return () => { alive = false; };
  }, [doc?.id, productionVersion]);
  if (!data || !doc) return <WorkspaceEmpty />;
  const changed = changedFields(doc, production).size;
  const shots = preview?.shots ?? doc.shots;
  const measured = shots.filter(s => !s.skip);
  const length = preview ? preview.segments.reduce((sum, g) => sum + segmentLength(g, preview.spacing), 0) : null;
  const next = data.progress.next;
  return <div className="wp-page"><WorkspaceBar /><Steps />
    <div className="wp-columns wp-defaults">
      <div className="wp-stack">
        <Panel title="默认检测与判定参数" detail="拍照点没单独设置时使用这里的值">
          <fieldset disabled={busy || !!data.workspace.pending} className="wp-plain-fieldset">
            <RecipeEditor key={doc.id + ":" + data.workspace.revision} initial={doc} production={production} onDraftChange={setDoc} />
          </fieldset>
        </Panel>
      </div>
      <div className="wp-stack">
        <section className="panel wp-panel wp-defaults-card" aria-label="配方摘要">
          <h3>配方摘要</h3>
          {shots.length ? <dl>
            <dt>拍照点</dt><dd>{shots.length}{shots.length > measured.length && <small> · 不检 {shots.length - measured.length}</small>}</dd>
            <dt>已示教</dt><dd>{measured.filter(shotTaught).length} / {measured.length}</dd>
            <dt>中线总长</dt><dd>{length == null ? "—" : length.toFixed(1) + " mm"}</dd>
            <dt>测量点</dt><dd>{preview ? preview.points.x.length : "—"}</dd>
          </dl> : <p className="muted">尚无拍照点，整圈采集后生成。</p>}
        </section>
        {next && <section className="wp-defaults-next" aria-label="下一步">
          <small>下一步 · {stageOf(next.step)?.label ?? "配方进度"}</small>
          <strong>{next.label}</strong>
          <Link to={targetPath(next.target)}>前往<ArrowRight size={14} aria-hidden="true" /></Link>
        </section>}
      </div>
    </div>
    <div className="wp-defaults-savebar" role="status" aria-label="候选保存状态">
      {productionVersion != null ? <span className={"wp-badge " + (changed ? "info" : "neutral")}>{changed ? `${changed} 项与生产 v${productionVersion} 不同` : `与生产 v${productionVersion} 一致`}</span> : <span className="wp-badge neutral">尚未发布</span>}
      <span className="muted">修改自动保存为候选配置，不影响在线生产配方</span>
      <span className="spacer" />
      {previewError ? <span className="wp-defaults-save warn"><AlertTriangle size={14} aria-hidden="true" />参数无效：{previewError}</span> : <span className={"wp-defaults-save " + saveState}>{saveState === "saved" ? <Check size={14} aria-hidden="true" /> : saveState === "failed" ? <AlertTriangle size={14} aria-hidden="true" /> : <LoaderCircle size={14} aria-hidden="true" />}{saveText[saveState]}</span>}
      {saveState === "failed" && <button className="btn small" onClick={retrySave}>重试保存</button>}
    </div>
  </div>;
}
