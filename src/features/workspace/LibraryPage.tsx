import { errorText } from "../../lib/errors";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Copy, Ellipsis, Package, PackageOpen, Plus, Search, Trash2 } from "lucide-react";
import { Button, Dialog, EmptyState, IconButton, Menu, NumberField, TextField } from "qing-steel";
import { desktopAvailable } from "../../lib/desktop";
import { recipeApi, type UnavailableRecipe } from "../cycle/api";
import type { RecipeDoc } from "../cycle/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel, WorkspaceBar } from "./components";
import { PackageExportDialog, PackageImportDialog } from "./PackageDialogs";
import { isCurrentFlow, type PackagePreview } from "./types";
import { recipeHref } from "./recipeRoutes";

export default function LibraryPage() {
  const {list,drafts,unavailable,selectedId,data,dirty,frameDirty,navigationDirty,segmentDirty,select,clearSelection,discardDrafts,reloadList,busy,setError} = useWorkspace();
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
  const [leaving,setLeaving]=useState<{label:string;run:()=>void}|null>(null);
  const [exporting,setExporting]=useState<string|null>(null);
  const [importing,setImporting]=useState<PackagePreview|null>(null);
  const pending=useRef(false);
  const current=useRef({alive:true,selectedId});current.current.selectedId=selectedId;
  useEffect(()=>{current.current.alive=true;return()=>{current.current.alive=false;};},[]);
  const available=desktopAvailable();
  const blocked=!available||busy||working;
  const unsaved=dirty||frameDirty||navigationDirty || segmentDirty;
  // 未保存的修改在哪一页：导航图与走廊草稿都在分段与示教页
  const resumePath=navigationDirty||segmentDirty||frameDirty?"/recipe/teach":"/recipe/progress";
  const unsavedParts=[segmentDirty&&"分段草稿",navigationDirty&&"工件导航图",frameDirty&&"走廊示教草稿",dirty&&"候选配置"].filter(Boolean).join("、");
  const guard=(label:string,run:()=>void)=>{if(unsaved)setLeaving({label,run});else run();};
  const ids = Array.from(new Set([...list.map(r => r.id),...drafts.map(w => w.doc.id)]));
  const duplicateId=(id:string)=>[...ids,...unavailable.map(r=>r.fileName.replace(/\.json$/i,""))].some(existing=>existing.toLowerCase()===id.toLowerCase());
  const usedCodes=new Set([...list.map(r=>r.productCode),...drafts.map(w=>w.doc.productCode)]);
  const identityError=(form:{id:string;name:string;productCode:number})=>!/^[A-Za-z0-9_-]{1,32}$/.test(form.id)?"配方编号只能包含字母、数字、下划线和短横线，最多 32 位":
    duplicateId(form.id)?"配方编号已存在，请换一个编号":!form.name.trim()?"请填写配方名称":
    !Number.isInteger(form.productCode)||form.productCode<1||form.productCode>32767?"型号需为 1–32767 的整数":
    usedCodes.has(form.productCode)?"型号已被其他配方使用":"";
  const invalid=creating?identityError(creating):"";
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
      let code = 1; while (code<=32767&&usedCodes.has(code)) code++;
      if(code>32767)throw new Error("型号已用完，删除不再使用的配方后重试");
      setCreatedId(null);
      setCreating({...template,id,name:copied ? copied.name + "（副本）" : "新配方",productCode:code,version:1,teachingId:null});
    } catch(e) { if(current.current.alive)setError(errorText(e)); } finally { pending.current=false;if(current.current.alive)setWorking(false); }
  };
  // 打开哪个配方只由地址决定（/recipe/<编号>/…），配方页按地址读候选
  const open=(id:string)=>{
    if(pending.current||blocked)return;
    // 继续有未保存修改的配方：回到修改所在的页面
    if(unsaved&&id===selectedId&&data?.workspace.doc.id===id){navigate(recipeHref(id,resumePath));return;}
    // 上次打开这个配方没读成（data 为空）：配方页不会重读同一编号，这里先重读
    if(id===selectedId&&data?.workspace.doc.id!==id)void select(id);
    navigate(recipeHref(id,"/recipe/progress"));
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
      setCreating(null);setCreatedId(null);navigate(recipeHref(id,"/recipe/progress"));
    } catch(e) { if(current.current.alive)setDialogError(errorText(e)); } finally { pending.current=false;if(current.current.alive)setWorking(false); }
  };
  const deletingStale = !!deleting && drafts.some(w => w.doc.id === deleting && !isCurrentFlow(w));
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
  const run=async(task:()=>Promise<void>)=>{
    if(pending.current||blocked)return;
    pending.current=true;setWorking(true);
    try{await task();}
    catch(e){if(current.current.alive)setError(errorText(e));}
    finally{pending.current=false;if(current.current.alive)setWorking(false);}
  };
  const pickPackage=()=>run(async()=>{const preview=await workspaceApi.packagePick();if(preview&&current.current.alive)setImporting(preview);});
  const afterImport=async(id:string)=>{await reloadList();if(current.current.selectedId===id)await select(id);};
  const visibleUnavailable = unavailable.filter(r => [r.fileName,r.id,r.name].join(" ").toLowerCase().includes(search.trim().toLowerCase()));
  const visible = ids.map(id => {
    const production = list.find(r => r.id === id);
    const draft = drafts.find(w => w.doc.id === id);
    return {id,production,draft,stale:!!draft&&!isCurrentFlow(draft),name:draft?.doc.name ?? production?.name ?? id};
  }).filter(r => (r.id + r.name + (r.draft?.doc.productCode ?? r.production?.productCode)).toLowerCase().includes(search.trim().toLowerCase()));
  return <div className="wp-page"><WorkspaceBar showNavigation={false} /><div className="wp-library-toolbar"><div className="wp-library-search"><TextField aria-label="搜索配方" placeholder="编号、名称或型号" leading={<Search size={16} aria-hidden="true" />} value={search} onChange={e => setSearch(e.target.value)} /></div><Button leadingIcon={<PackageOpen size={16} />} disabled={blocked} title="导入工件配方包，替换本机同编号的候选" onClick={() => guard("导入工件配方",()=>void pickPackage())}>导入工件配方</Button><Button variant="filled" leadingIcon={<Plus size={16} />} disabled={!desktopAvailable() || busy || working} onClick={() => guard("新建配方",()=>void prepare())}>新建配方</Button></div>
    {!desktopAvailable() && <Notice title="配方操作需要桌面后端" tone="warn">本页不会在浏览器中创建、保存或发布生产配置。</Notice>}
    <div className="wp-library-grid">{visible.map(r => <Panel key={r.id} title={r.name} hoverEffect={!r.stale && !blocked} className="wp-recipe-card" actions={<Menu label={`${r.id} 配方操作`} align="end"
      trigger={<IconButton aria-label={`更多操作 ${r.id}`} title="更多操作" disabled={blocked}><Ellipsis size={18} /></IconButton>}
      items={[
        ...(!r.stale ? [{ id: "copy", label: "复制配方", icon: <Copy size={16} />, onSelect: () => guard("复制 "+r.id,()=>void prepare(r.id)) },
          ...(r.draft ? [{ id: "export", label: "导出工件配方", icon: <Package size={16} />, disabled: !!r.draft.pending, onSelect: () => guard("导出工件配方 "+r.id,()=>setExporting(r.id)) }] : [])] : []),
        { id: "delete", label: "删除配方", icon: <Trash2 size={16} />, danger: true, disabled: !!r.draft?.pending, onSelect: () => {setDeleting(r.id);setRemovedId(null);setDialogError("");} },
      ]} />}>
      <p className="wp-recipe-identity muted"><span>{r.id}</span><span aria-hidden="true"> · </span>{`PLC 型号 ${r.production?.productCode ?? r.draft?.doc.productCode}`}</p>
      <p className="wp-recipe-versions"><span className={r.production ? "" : "c-warn"}>{r.production ? `${r.production.stage === "collect" ? "采集版" : "已发布"} v${r.production.version}` : "未发布"}</span></p>
      {r.draft?.pending && <Badge tone="warn">等待生效</Badge>}
      {r.stale ? <p className="c-warn">旧流程候选已停用，请删除后重建。</p> : !(r.draft ? r.draft.teachingPiece : r.production?.teachingPiece) && <p className="wp-recipe-notice muted">尚未选择示教件</p>}
      {!r.stale && <div className="wp-actions wp-recipe-open"><Button variant="filled" size="sm" disabled={blocked} onClick={()=>r.id===selectedId?open(r.id):guard("打开 "+r.id,()=>open(r.id))}>打开配方</Button></div>}
    </Panel>)}{visibleUnavailable.map(r => <Panel key={"unavailable:"+r.fileName} title={r.name || r.id || r.fileName} detail={r.fileName} className="wp-recipe-card" actions={<Badge tone="warn">不可用</Badge>}>
      <p className="muted">文件格式 {r.schemaVersion ?? "未知"} · 当前格式 {r.currentSchemaVersion}</p>
      <p>{r.reason}</p><p className="muted">此文件无法用于配置或生产。</p>
      <div className="wp-actions"><Button danger leadingIcon={<Trash2 size={16} />} disabled={blocked} onClick={()=>{setDeletingUnavailable(r);setUnavailableRemoved(false);setDialogError("");}}>删除文件</Button></div>
    </Panel>)}</div>{!visible.length && !visibleUnavailable.length && <EmptyState icon={<Search size={30} aria-hidden="true" />} title={ids.length || unavailable.length ? "没有匹配的配方" : "尚未加载配方"} description={ids.length || unavailable.length ? "修改搜索条件后重试。" : "桌面软件会从本地配方库读取已有配置。"} />}
    {exporting && <PackageExportDialog id={exporting} onClose={()=>setExporting(null)} />}
    {importing && <PackageImportDialog preview={importing} onClose={()=>setImporting(null)} onImported={afterImport} onOpen={id=>{setImporting(null);void open(id);}} />}
    {leaving && <Dialog open title={`配方 ${selectedId} 有未保存的修改`} onOpenChange={open=>{if(!open)setLeaving(null);}} footer={<><Button onClick={()=>setLeaving(null)}>取消</Button><Button danger disabled={blocked} onClick={()=>{if(!discardDrafts())return;const next=leaving;setLeaving(null);next.run();}}>放弃修改并{leaving.label}</Button><Button variant="filled" onClick={()=>{setLeaving(null);if(selectedId)navigate(recipeHref(selectedId,resumePath));}}>回到 {selectedId} 继续编辑</Button></>}><p>{unsavedParts}尚未保存。可以回到 {selectedId} 保存或撤销，也可以放弃这些修改后{leaving.label}；候选里已保存的内容不受影响。</p></Dialog>}
    {deletingUnavailable && <Dialog open title="删除不可用配方文件" closeDisabled={working} onOpenChange={open=>{if(!open)setDeletingUnavailable(null);}} footer={<><Button disabled={working} onClick={()=>setDeletingUnavailable(null)}>取消</Button><Button danger disabled={blocked} onClick={()=>void removeUnavailable()}>{working ? "处理中…" : unavailableRemoved ? "刷新配方库" : "确认删除文件"}</Button></>}><p>将永久删除文件 {deletingUnavailable.fileName}。此操作不可撤销，同编号的候选、示教资料及历史记录保留。</p>{dialogError && <Notice title="操作未完成" tone="warn">{dialogError}</Notice>}</Dialog>}
    {creating && <Dialog open title="建立候选配方" closeDisabled={working} onOpenChange={open=>{if(!open)setCreating(null);}} footer={<><Button disabled={working} onClick={() => setCreating(null)}>取消</Button><Button variant="filled" disabled={blocked || (!createdId&&!!invalid)} onClick={() => void create()}>{working ? "创建中…" : createdId?"打开已创建候选":"创建候选"}</Button></>}><div className="wp-stack"><TextField label="配方编号" className="mono" disabled={working||!!createdId} value={creating.id} maxLength={32} onChange={e => setCreating({...creating,id:e.target.value})} /><TextField label="配方名称" disabled={working||!!createdId} value={creating.name} onChange={e => setCreating({...creating,name:e.target.value})} /><NumberField label="型号" disabled={working||!!createdId} min={1} max={32767} value={Number.isFinite(creating.productCode)?creating.productCode:""} onValueChange={v => setCreating({...creating,productCode:v===""?NaN:v})} />{invalid&&!createdId&&<p className="form-error" role="alert">{invalid}</p>}<Notice title="先建立候选，示教后发布">新候选不会被 PLC 或在线检测自动使用。</Notice>{dialogError && <Notice title="无法创建" tone="warn">{dialogError}</Notice>}</div></Dialog>}
    {deleting && <Dialog open title="删除配方" closeDisabled={working} onOpenChange={open=>{if(!open)setDeleting(null);}} footer={<><Button disabled={working} onClick={() => setDeleting(null)}>取消</Button><Button danger disabled={blocked} onClick={() => void remove()}>{removedId?"刷新配方库":"删除 "+deleting}</Button></>}><p>{deletingStale ? `将删除此旧流程候选及其导入的采集轮次${list.some(r => r.id === deleting) ? "；同编号的生产配方照常运行，删除后可从它新建候选" : ""}。已存储的历史结果和配方快照保留。` : "将删除此配方的生产配置、候选及示教资料。已存储的历史结果和配方快照保留。"}</p>{dialogError && <Notice title="无法删除" tone="warn">{dialogError}</Notice>}</Dialog>}
  </div>;
}
