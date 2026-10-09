import { useEffect, useState } from "react";
import { Copy, Plus, Trash2 } from "lucide-react";
import { cameraApi } from "../features/camera";
import { recipeApi, useLayout, type RecipeDoc, type RecipeSummary } from "../features/cycle";
import { triggerModeLabel } from "../features/history";
import { FlyshotTeach, RecipeEditor } from "../features/recipe";

export default function RecipePage() {
  const [list, setList] = useState<RecipeSummary[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditing] = useState<{ doc: RecipeDoc; originalId: string | null; key: number } | null>(null);
  const [cameras, setCameras] = useState<{ id: string; name: string }[]>([]);
  const [error, setError] = useState("");

  /** select：要打开的配方；不给时留在当前这个，null 表示刚删掉当前配方、打开列表里的第一个。 */
  const reload = async (select?: string | null) => {
    const r = await recipeApi.list();
    setList(r.recipes);
    setErrors(r.errors);
    const id = (select === undefined ? selected : select) ?? r.recipes[0]?.id ?? null;
    if (id) open(id);
  };

  const open = async (id: string) => {
    setError("");
    try {
      const doc = await recipeApi.doc(id);
      setSelected(id);
      setEditing({ doc, originalId: id, key: Date.now() });
    } catch (e) {
      setError(String(e));
    }
  };

  useEffect(() => {
    reload();
    cameraApi.rigConfig().then((c) => setCameras(c.map(({ id, name }) => ({ id, name }))));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const create = async () => {
    try {
      const doc = await recipeApi.template();
      setSelected(null);
      setEditing({ doc, originalId: null, key: Date.now() });
    } catch (e) {
      setError(String(e));
    }
  };
  const copy = () => {
    if (!editing) return;
    const code = Math.max(0, ...list.map((r) => r.productCode)) + 1;
    setSelected(null);
    setEditing({ doc: { ...editing.doc, id: `${editing.doc.id}-COPY`.slice(0, 32), name: `${editing.doc.name}（副本）`, productCode: code, version: 1 }, originalId: null, key: Date.now() });
  };
  const remove = async () => {
    if (!selected || !window.confirm(`删除配方 ${selected}？已有检测记录里的配方快照不受影响。`)) return;
    try {
      await recipeApi.remove(selected);
      setSelected(null);
      setEditing(null);
      reload(null);
    } catch (e) {
      setError(String(e));
    }
  };

  const saved = useLayout(editing?.originalId, list.find((r) => r.id === editing?.originalId)?.hash, true);

  return (
    <div className="rcp-page">
      <div className="panel rcp-list">
        <div className="panel-head">
          <h3 className="panel-title">配方</h3>
        </div>
        <div className="rcp-items">
          {list.map((r) => (
            <button key={r.id} className={`rcp-item${r.id === selected ? " active" : ""}`} onClick={() => open(r.id)}>
              <b className="mono">{r.id}</b>
              <span>{r.name}</span>
              <span className="muted mono">
                代码 {r.productCode} · v{r.version} · {triggerModeLabel(r.triggerMode)} N={r.shotCount}
              </span>
            </button>
          ))}
        </div>
        <div className="rcp-actions">
          <button className="btn" onClick={() => create()}>
            <Plus size={15} />
            新建飞拍
          </button>
          <button className="btn" onClick={copy} disabled={!editing}>
            <Copy size={15} />
            复制
          </button>
          <button className="btn" onClick={remove} disabled={!selected}>
            <Trash2 size={15} />
            删除
          </button>
        </div>
        {errors.map((e) => (
          <div key={e} className="notice error">
            {e}
          </div>
        ))}
        {error && <div className="notice error">{error}</div>}
        <p className="muted hint">配方存在数据目录的 recipes 下，每个一个 JSON 文件。内容变了保存时版本号自动 +1，检测记录按内容哈希存快照。</p>
      </div>
      <div className="rcp-main">
        {editing && (
          <div className="panel">
            <RecipeEditor key={editing.key} initial={editing.doc} originalId={editing.originalId} cameras={cameras} onSaved={(id) => reload(id)} />
          </div>
        )}
        {saved && <FlyshotTeach recipe={saved} />}
      </div>
    </div>
  );
}
