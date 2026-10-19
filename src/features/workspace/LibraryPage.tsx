import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, Plus, Search, Trash2 } from "lucide-react";
import { Button, Dialog, EmptyState, IconButton, NumberField, TextField } from "qing-steel";
import { desktopAvailable } from "../../lib/desktop";
import { recipeApi, type UnavailableRecipe } from "../cycle/api";
import type { RecipeDoc } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel, WorkspaceBar } from "./components";

export default function LibraryPage() {
  const {list,drafts,unavailable,selectedId,select,clearSelection,reloadList,busy,setError} = useWorkspace();
  const navigate = useNavigate();
  const [search,setSearch] = useState("");
  const [creating,setCreating] = useState<RecipeDoc | null>(null);
  const [working,setWorking] = useState(false);
  const [dialogError,setDialogError] = useState("");
  const [deleting,setDeleting] = useState<string | null>(null);
  const [deletingUnavailable,setDeletingUnavailable] = useState<UnavailableRecipe | null>(null);
  const [unavailableRemoved,setUnavailableRemoved] = useState(false);
  const [createdId,setCreatedId]=useState<string|null>(null);
  const [removedId,setRemovedId]=useState<string|null>(null);
  const pending=useRef(false);
  const current=useRef({alive:true,selectedId});current.current.selectedId=selectedId;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  const available=desktopAvailable();
  const blocked=!available||busy||working;
  const ids = Array.from(new Set([...list.map(r => r.id),...drafts.map(w => w.doc.id)]));
  const duplicateId=(id:string)=>[...ids,...unavailable.map(r=>r.fileName.replace(/\.json$/i,""))].some(existing=>existing.toLowerCase()===id.toLowerCase());
  const usedCodes=new Set([...list.map(r=>r.productCode),...drafts.map(w=>w.doc.productCode)]);
  const invalid=creating? !/^[A-Za-z0-9_-]{1,32}$/.test(creating.id)?"配方编号只能包含字母、数字、下划线和短横线，最多 32 位":
    duplicateId(creating.id)?"配方编号已存在，请换一个编号":!creating.name.trim()?"请填写配方名称":
    !Number.isInteger(creating.productCode)||creating.productCode<1||creating.productCode>65535?"产品代码需为 1–65535 的整数":
    usedCodes.has(creating.productCode)?"产品代码已被其他配方使用":"":"";
  const prepare = async (copyId?: string) => {
    if(pending.current||blocked)return;
    pending.current=true;
    setWorking(true); setDialogError("");
    try {
      const copied = copyId ? (drafts.find(w => w.doc.id === copyId)?.doc ?? await recipeApi.doc(copyId)) : null;
      const template = copied ? structuredClone(copied) : await recipeApi.template();
      if(!current.current.alive)return;
      let id = copied ? copied.id.slice(0,25) + "-COPY" : template.id;
      let suffix = 2;
      while (duplicateId(id)) id = (copied ? copied.id.slice(0,22) + "-COPY-" : "NEW-") + suffix++;
      let code = 1; while (code<=65535&&usedCodes.has(code)) code++;
      if(code>65535)throw new Error("产品代码已用完，删除不再使用的配方后重试");
      setCreatedId(null);
      setCreating({...template,shots:copied ? template.shots : [],id,name:copied ? copied.name + "（副本）" : "新配方",productCode:code,version:1,teachingId:null});
    } catch(e) { if(current.current.alive)setError(errorText(e)); } finally { pending.current=false;if(current.current.alive)setWorking(false); }
  };
  const open=async(id:string)=>{
    if(pending.current||blocked)return;
    pending.current=true;setWorking(true);
    try { const selected=await select(id);if(selected?.workspace.doc.id===id&&current.current.alive)navigate("/recipe/progress"); }
    catch(error){if(current.current.alive)setError(errorText(error));}
    finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  const create = async () => {
    if (!creating || pending.current||blocked||(!createdId&&invalid)) return;
    pending.current=true;
    setWorking(true); setDialogError("");
    try {
      const id=createdId??(await workspaceApi.create({...creating,name:creating.name.trim()})).workspace.doc.id;
      if(!current.current.alive)return;
      setCreatedId(id);
      await reloadList();if(!current.current.alive)return;
      const selected=await select(id);if(!current.current.alive)return;
      if(selected?.workspace.doc.id!==id){setDialogError("候选已创建，选用未完成。修正加载错误后重试打开候选。");return;}
      setCreating(null);setCreatedId(null);navigate("/recipe/progress");
    } catch(e) { if(current.current.alive)setDialogError(errorText(e)); } finally { pending.current=false;if(current.current.alive)setWorking(false); }
  };
  const remove = async () => {
    if (!deleting || pending.current||blocked) return;
    pending.current=true;
    setWorking(true); setDialogError("");
    try {
      if(removedId!==deleting)await workspaceApi.remove(deleting);
      if(!current.current.alive)return;
      setRemovedId(deleting);
      await reloadList();if(!current.current.alive)return;
      if(current.current.selectedId===deleting){
        const next = ids.find(id => id !== deleting);
        if (next) await select(next);
        else clearSelection();
      }
      if(current.current.alive){setDeleting(null);setRemovedId(null);}
    } catch(e) { if(current.current.alive)setDialogError(errorText(e)); } finally { pending.current=false;if(current.current.alive)setWorking(false); }
  };
  const removeUnavailable = async () => {
    if (!deletingUnavailable || pending.current || blocked) return;
    pending.current=true;setWorking(true);setDialogError("");
    try {
      if (!unavailableRemoved) await recipeApi.removeUnavailable(deletingUnavailable.fileName);
      if (!current.current.alive) return;
      setUnavailableRemoved(true);
      await reloadList();
      if (current.current.alive) { setDeletingUnavailable(null);setUnavailableRemoved(false); }
    } catch(e) { if(current.current.alive)setDialogError(errorText(e)); }
    finally { pending.current=false;if(current.current.alive)setWorking(false); }
  };
  const visibleUnavailable = unavailable.filter(r => [r.fileName,r.id,r.name].join(" ").toLowerCase().includes(search.trim().toLowerCase()));
  const visible = ids.map(id => {
    const production = list.find(r => r.id === id);
    const draft = drafts.find(w => w.doc.id === id);
    return {id,production,draft,name:draft?.doc.name ?? production?.name ?? id};
  }).filter(r => (r.id + r.name + (r.draft?.doc.productCode ?? r.production?.productCode)).toLowerCase().includes(search.trim().toLowerCase()));
  return <div className="wp-page"><WorkspaceBar /><div className="wp-actions"><TextField className="wp-library-search" aria-label="搜索配方" placeholder="编号、名称或产品代码" leading={<Search size={16} aria-hidden="true" />} value={search} onChange={e => setSearch(e.target.value)} /><span className="spacer" /><Button variant="filled" leadingIcon={<Plus size={16} />} disabled={!desktopAvailable() || busy || working} onClick={() => void prepare()}>新建配方</Button></div>
    {!desktopAvailable() && <Notice title="配方操作需要桌面后端" tone="warn">本页不会在浏览器中创建、保存或发布生产配置。</Notice>}
    <div className="wp-library-grid">{visible.map(r => <Panel key={r.id} title={r.name} detail={r.id} className={"wp-recipe-card " + (r.id === selectedId ? "selected" : "")} actions={<Badge tone="neutral">飞拍</Badge>}>
      <div className="wp-actions"><Badge tone={r.production ? "ok" : "warn"}>{r.production ? "已发布 v" + r.production.version : "未发布"}</Badge>{r.draft && <Badge>候选 v{r.draft.doc.version}</Badge>}{r.draft?.pending && <Badge tone="warn">待生效</Badge>}</div><p className="muted mono">代码 {r.draft?.doc.productCode ?? r.production?.productCode} · {(r.draft?.doc.shots.length ?? r.production?.shotCount) + " 个拍照点"}</p>
      <div className="wp-actions"><Button variant="filled" disabled={blocked} onClick={()=>void open(r.id)}>继续配置</Button><IconButton title="复制配方" aria-label={"复制配方 " + r.id} disabled={blocked} onClick={() => void prepare(r.id)}><Copy size={16} /></IconButton><IconButton title="删除配方" aria-label={"删除配方 " + r.id} disabled={blocked || !!r.draft?.pending} onClick={() => {setDeleting(r.id);setRemovedId(null);setDialogError("");}}><Trash2 size={16} /></IconButton></div>
    </Panel>)}{visibleUnavailable.map(r => <Panel key={"unavailable:"+r.fileName} title={r.name || r.id || r.fileName} detail={r.fileName} className="wp-recipe-card" actions={<Badge tone="warn">不可用</Badge>}>
      <p className="muted">文件格式 {r.schemaVersion ?? "未知"} · 当前格式 {r.currentSchemaVersion}</p>
      <p>{r.reason}</p><p className="muted">仅展示文件信息，不能配置、试测、发布或用于生产。升级功能暂未实现。</p>
      <div className="wp-actions"><Button disabled title="升级功能暂未实现">升级（暂未实现）</Button><Button danger leadingIcon={<Trash2 size={16} />} disabled={blocked} onClick={()=>{setDeletingUnavailable(r);setUnavailableRemoved(false);setDialogError("");}}>删除文件</Button></div>
    </Panel>)}</div>{!visible.length && !visibleUnavailable.length && <EmptyState icon={<Search size={30} aria-hidden="true" />} title={ids.length || unavailable.length ? "没有匹配的配方" : "尚未加载配方"} description={ids.length || unavailable.length ? "修改搜索条件后重试。" : "桌面软件会从本地配方库读取已有配置。"} />}
    {deletingUnavailable && <Dialog open title="删除不可用配方文件" closeDisabled={working} onOpenChange={open=>{if(!open)setDeletingUnavailable(null);}} footer={<><Button disabled={working} onClick={()=>setDeletingUnavailable(null)}>取消</Button><Button danger disabled={blocked} onClick={()=>void removeUnavailable()}>{working ? "处理中…" : unavailableRemoved ? "刷新配方库" : "确认删除文件"}</Button></>}><p>将永久删除文件 {deletingUnavailable.fileName}。此操作不可撤销，同编号的候选、示教资料及历史记录保留。</p>{dialogError && <Notice title="操作未完成" tone="warn">{dialogError}</Notice>}</Dialog>}
    {creating && <Dialog open title="建立候选配方" closeDisabled={working} onOpenChange={open=>{if(!open)setCreating(null);}} footer={<><Button disabled={working} onClick={() => setCreating(null)}>取消</Button><Button variant="filled" disabled={blocked || (!createdId&&!!invalid)} onClick={() => void create()}>{working ? "创建中…" : createdId?"打开已创建候选":"创建候选"}</Button></>}><div className="wp-stack"><TextField label="配方编号" className="mono" disabled={working||!!createdId} value={creating.id} maxLength={32} onChange={e => setCreating({...creating,id:e.target.value})} /><TextField label="配方名称" disabled={working||!!createdId} value={creating.name} onChange={e => setCreating({...creating,name:e.target.value})} /><NumberField label="产品代码" disabled={working||!!createdId} min={1} max={65535} value={Number.isFinite(creating.productCode)?creating.productCode:""} onValueChange={v => setCreating({...creating,productCode:v===""?NaN:v})} />{invalid&&!createdId&&<p className="form-error" role="alert">{invalid}</p>}<Notice title="先建立候选，验证后发布">新候选不会被 PLC 或在线检测自动使用。</Notice>{dialogError && <Notice title="无法创建" tone="warn">{dialogError}</Notice>}</div></Dialog>}
    {deleting && <Dialog open title="删除配方" closeDisabled={working} onOpenChange={open=>{if(!open)setDeleting(null);}} footer={<><Button disabled={working} onClick={() => setDeleting(null)}>取消</Button><Button danger disabled={blocked} onClick={() => void remove()}>{removedId?"刷新配方库":"删除 "+deleting}</Button></>}><p>将删除此配方的生产配置、候选及示教资料。已存储的历史结果和配方快照保留。</p>{dialogError && <Notice title="无法删除" tone="warn">{dialogError}</Notice>}</Dialog>}
  </div>;
}
