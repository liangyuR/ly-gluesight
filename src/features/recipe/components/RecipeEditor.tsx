import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ArrowDown, ArrowUp, Plus, Save, Trash2, Upload } from "lucide-react";
import { recipeApi } from "../../cycle/api";
import TrajectoryMap from "../../cycle/components/TrajectoryMap";
import type { JudgeParams, PathSpec, Recipe, RecipeDoc, SegmentLimits, ShotSpec } from "../../cycle/types";

interface Props {
  initial: RecipeDoc;
  originalId: string | null;
  /** 相机组里的相机：配方按编号引用 */
  cameras: { id: string; name: string }[];
  onSaved: (id: string) => void;
  onDraftChange?: (doc: RecipeDoc) => void;
  saveCandidate?: (doc: RecipeDoc) => Promise<boolean>;
}

const paramFields: [keyof JudgeParams, string][] = [
  ["nominal", "名义"],
  ["tolUpper", "上公差"],
  ["tolLower", "下公差"],
  ["absMin", "绝对下限"],
  ["absMax", "绝对上限"],
  ["maxExcursionLen", "允许超差长度"],
];

const cells = (line: string) =>
  line
    .split(/[,;\t ]/)
    .map((c) => c.trim())
    .filter(Boolean);

/** 按 Rust 的 f32 解析认数（nan、inf 也算数），解不了返回 null。 */
function num(c: string): number | null {
  if (/^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(c)) return Number(c);
  const m = /^([+-]?)(inf|infinity|nan)$/i.exec(c);
  return m ? (m[2].toLowerCase() === "nan" ? NaN : m[1] === "-" ? -Infinity : Infinity) : null;
}

/** 每行开头连续的数（遇到不是数的就停），前两个是有限数才算一个点。 */
function parseRows(text: string): number[][] {
  return text
    .split(/\r?\n/)
    .map((l) => {
      const out: number[] = [];
      for (const c of cells(l)) {
        const v = num(c);
        if (v === null) break;
        out.push(v);
      }
      return out;
    })
    .filter((v) => v.length >= 2 && Number.isFinite(v[0]) && Number.isFinite(v[1]));
}

/** 有圆弧时首行写表头 "x, y, bulge"。 */
const pathText = (pts: [number, number][], bulges: number[] = []) => {
  const arcs = bulges.some((b) => b);
  const rows = pts.map(([x, y], i) => (arcs ? `${x}, ${y}, ${Number((bulges[i] ?? 0).toFixed(6))}` : `${x}, ${y}`));
  return (arcs ? ["x, y, bulge", ...rows] : rows).join("\n");
};

/** 与导入 CSV（recipe.rs 的 csv_path）同一个规矩。 */
function parsePathText(text: string): { points: [number, number][]; bulges: number[] } {
  const first = text.split(/\r?\n/).find((l) => cells(l).length > 0);
  const head = first ? cells(first) : [];
  const col = head.some((c) => num(c) === null) ? head.findIndex((c) => c.toLowerCase() === "bulge") : -1;
  const rows = parseRows(text);
  const bulges = rows.map((v) => (col >= 0 && Number.isFinite(v[col]) ? v[col] : 0));
  return { points: rows.map((v) => [v[0], v[1]] as [number, number]), bulges: bulges.some((b) => b !== 0) ? bulges : [] };
}

function Num({ label, value, onChange, step = 0.1, hint }: { label: string; value: number; onChange: (v: number) => void; step?: number; hint?: string }) {
  return (
    <label className="field" title={hint}>
      <span>{label}</span>
      <input className="input mono" type="number" step={step} value={Number.isFinite(value) ? value : ""} onChange={(e) => onChange(e.target.value===""?NaN:Number(e.target.value))} />
    </label>
  );
}

function Section({ title, children, extra }: { title: string; children: ReactNode; extra?: ReactNode }) {
  return (
    <section className="rcp-section">
      <div className="panel-head">
        <h4 className="sub-title">{title}</h4>
        <span className="spacer" />
        {extra}
      </div>
      {children}
    </section>
  );
}

function LimitsTable({ label, value, onChange, widthDefault }: { label: string; value: SegmentLimits; onChange: (v: SegmentLimits) => void; widthDefault: JudgeParams }) {
  const row = (name: string, p: JudgeParams, set: (p: JudgeParams) => void, extra?:ReactNode) => (
    <tr>
      <td>{name}{extra}</td>
      {paramFields.map(([k,field]) => (
        <td key={k}>
          <input aria-label={`${name} · ${field}`} className="input mono" type="number" step={0.05} value={Number.isFinite(p[k])?p[k]:""} onChange={(e) => set({ ...p, [k]: e.target.value===""?NaN:Number(e.target.value) })} />
        </td>
      ))}
    </tr>
  );
  return (
    <>
      {row(`${label} · 位置`, value.position, (position) => onChange({ ...value, position }))}
      {value.width ? (
        row(`${label} · 胶宽`, value.width, (width) => onChange({ ...value, width }),<button className="btn small" onClick={()=>onChange({...value,width:null})}>停用胶宽判定</button>)
      ) : (
        <tr>
          <td>{label} · 胶宽</td>
          <td colSpan={paramFields.length}>
            <button className="btn small" onClick={() => onChange({ ...value, width: widthDefault })}>
              启用胶宽判定
            </button>
          </td>
        </tr>
      )}
    </>
  );
}

/** 新拍照点的编号：P1、P2… 里第一个没用过的。 */
export function nextShotId(shots: ShotSpec[]) {
  const used = new Set(shots.map((s) => s.id));
  let n = 1;
  while (used.has(`P${n}`)) n++;
  return `P${n}`;
}

/** 胶路包围盒中心，第一个拍照点的默认位置。 */
function pathCenter(path: PathSpec): [number, number] {
  if (path.kind === "roundedRect") return [path.width / 2, path.height / 2];
  const pts = path.points.filter((p) => p.every(Number.isFinite));
  if (!pts.length) return [0, 0];
  const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
  return [(Math.min(...xs) + Math.max(...xs)) / 2, (Math.min(...ys) + Math.max(...ys)) / 2];
}

/** 新加的拍照点：相机与中心沿用上一行，Pose 同编号；视野、标定留空用缺省。 */
export function newShot(shots: ShotSpec[], cameras: { id: string }[], path: PathSpec): ShotSpec {
  const id = nextShotId(shots), last = shots.at(-1);
  return { id, poseId: id, camera: last?.camera ?? cameras[0]?.id ?? "", center: last ? [...last.center] : pathCenter(path) };
}

/** 改拍照点视野的一边：留空表示用配方视野；另一边也没单独设时整项去掉。 */
function shotFovWith(shot: ShotSpec, fov: [number, number], i: 0 | 1, v: number): [number, number] | undefined {
  const other = 1 - i;
  if (Number.isNaN(v)) {
    if (!shot.fov || shot.fov[other] === fov[other]) return undefined;
    v = fov[i];
  }
  const next: [number, number] = shot.fov ? [...shot.fov] : [...fov];
  next[i] = v;
  return next;
}

function ShotTable({ shots, fov, cameras, path, onChange }: { shots: ShotSpec[]; fov: [number, number]; cameras: { id: string; name: string }[]; path: PathSpec; onChange: (shots: ShotSpec[]) => void }) {
  const edit = (k: number, patch: Partial<ShotSpec>) =>
    onChange(
      shots.map((s, i) => {
        if (i !== k) return s;
        const next = { ...s, ...patch };
        // 后端不认 null：没单独设的视野、标定不发送
        if (next.fov === undefined) delete next.fov;
        if (!next.calib) delete next.calib;
        return next;
      }),
    );
  const move = (k: number, to: number) => {
    const next = [...shots];
    [next[k], next[to]] = [next[to], next[k]];
    onChange(next);
  };
  const coord = (v: number) => (Number.isFinite(v) ? v : "");
  const number = (value: string) => (value === "" ? NaN : Number(value));
  return (
    <>
      <div className="table-wrap">
        <table className="table rcp-shots">
          <thead>
            <tr>
              <th>序号</th>
              <th>编号</th>
              <th>Pose</th>
              <th>相机</th>
              <th>中心 X / Y（mm）</th>
              <th>视野 宽 × 高（mm）</th>
              <th>标定引用</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {shots.map((s, k) => {
              const row = `拍照点 ${k + 1}`;
              return (
                <tr key={k}>
                  <td className="mono muted">{k + 1}</td>
                  <td>
                    <input aria-label={`${row} · 编号`} className="input mono rcp-shot-id" value={s.id} onChange={(e) => edit(k, { id: e.target.value.trim() })} />
                  </td>
                  <td>
                    <input aria-label={`${row} · Pose`} className="input mono rcp-shot-id" value={s.poseId} onChange={(e) => edit(k, { poseId: e.target.value })} />
                  </td>
                  <td>
                    <select aria-label={`${row} · 相机`} className="input" value={s.camera} onChange={(e) => edit(k, { camera: e.target.value })}>
                      {!cameras.some((c) => c.id === s.camera) && <option value={s.camera}>{s.camera}（不在相机组里）</option>}
                      {cameras.map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.name} · {c.id}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td>
                    <span className="rcp-pair">
                      <input aria-label={`${row} · 中心 X（mm）`} className="input mono" type="number" step={1} value={coord(s.center[0])} onChange={(e) => edit(k, { center: [number(e.target.value), s.center[1]] })} />
                      <input aria-label={`${row} · 中心 Y（mm）`} className="input mono" type="number" step={1} value={coord(s.center[1])} onChange={(e) => edit(k, { center: [s.center[0], number(e.target.value)] })} />
                    </span>
                  </td>
                  <td>
                    <span className="rcp-pair">
                      {([0, 1] as const).map((i) => (
                        <input
                          key={i}
                          aria-label={`${row} · 视野${i ? "高" : "宽"}（mm）`}
                          className="input mono"
                          type="number"
                          step={1}
                          placeholder={String(fov[i])}
                          value={s.fov && Number.isFinite(s.fov[i]) ? s.fov[i] : ""}
                          onChange={(e) => edit(k, { fov: shotFovWith(s, fov, i, number(e.target.value)) })}
                        />
                      ))}
                    </span>
                  </td>
                  <td>
                    <input aria-label={`${row} · 标定引用`} className="input mono rcp-shot-id" placeholder={s.camera} value={s.calib ?? ""} onChange={(e) => edit(k, { calib: e.target.value.trim() || undefined })} />
                  </td>
                  <td>
                    <span className="rcp-row-actions">
                      <button type="button" className="icon-btn" aria-label={`上移${row}`} disabled={k === 0} onClick={() => move(k, k - 1)}>
                        <ArrowUp size={14} />
                      </button>
                      <button type="button" className="icon-btn" aria-label={`下移${row}`} disabled={k === shots.length - 1} onClick={() => move(k, k + 1)}>
                        <ArrowDown size={14} />
                      </button>
                      <button type="button" className="icon-btn" aria-label={`删除${row}`} onClick={() => onChange(shots.filter((_, i) => i !== k))}>
                        <Trash2 size={14} />
                      </button>
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <div className="rcp-actions">
        <button type="button" className="btn small" disabled={shots.length >= 64} onClick={() => onChange([...shots, newShot(shots, cameras, path)])}>
          <Plus size={14} />
          添加拍照点
        </button>
        <span className="muted hint">按拍照顺序排列。视野留空用上面的默认视野，标定引用留空用该相机的工位标定；同一 Pose 可以触发两台相机。</span>
      </div>
    </>
  );
}

export default function RecipeEditor({ initial, originalId, cameras, onSaved, onDraftChange, saveCandidate }: Props) {
  const [doc, setDoc] = useState<RecipeDoc>(initial);
  const [preview, setPreview] = useState<Recipe | null>(null);
  const [previewDoc,setPreviewDoc]=useState<RecipeDoc|null>(null);
  const [previewError, setPreviewError] = useState("");
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [polyText, setPolyText] = useState(initial.path.kind === "polyline" ? pathText(initial.path.points, initial.path.bulges) : "");
  const [importing,setImporting]=useState(false),[saving,setSaving]=useState(false);
  const mounted=useRef(true),importSerial=useRef(0),pendingSave=useRef(false);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;importSerial.current++;};},[]);
  useEffect(() => { onDraftChange?.(doc); }, [doc, onDraftChange]);

  useEffect(() => {
    let alive = true;
    setPreviewError("");
    const t = setTimeout(() => {
      recipeApi
        .preview(doc)
        .then((r) => {
          if (!alive) return;
          setPreview(r);
          setPreviewDoc(doc);
          setPreviewError("");
        })
        .catch((e) => { if (alive) {setPreview(null);setPreviewDoc(null);setPreviewError(String(e));} });
    }, 350);
    return () => { alive = false; clearTimeout(t); };
  }, [doc]);

  const set = <K extends keyof RecipeDoc>(k: K, v: RecipeDoc[K]) => {setNotice(null);setDoc(previous=>({ ...previous, [k]: v }));};
  const setPath = (p: PathSpec) => {importSerial.current++;setImporting(false);set("path", p);};
  // 新加胶宽限值时的默认值，按名义胶宽 2 mm
  const bead = 2;
  const widthDefault: JudgeParams = { nominal: bead, tolUpper: 0.35 * bead, tolLower: 0.3 * bead, absMin: 0.4 * bead, absMax: 1.9 * bead, maxExcursionLen: 3 };

  const importFile = async (file: File) => {
    const serial=++importSerial.current;
    setImporting(true);setNotice(null);
    try {
      if(!/\.(csv|txt|dxf)$/i.test(file.name))throw new Error("胶路导入支持 CSV、TXT 或 DXF 文件");
      if(!file.size)throw new Error("导入文件为空");
      if(file.size>5*1024*1024)throw new Error("胶路文件不能超过 5 MB");
      const text=await file.text();
      if(!mounted.current||serial!==importSerial.current)return;
      const r = await recipeApi.parsePath(text, file.name);
      if(!mounted.current||serial!==importSerial.current)return;
      setPolyText(pathText(r.points, r.bulges));
      setDoc(previous=>({...previous,path:{ kind: "polyline", points: r.points, bulges: r.bulges, closed: r.closed, radius: previous.path.kind === "polyline" ? previous.path.radius : 0 }}));
      const arcs = r.bulges.filter((b) => b !== 0).length;
      setNotice({
        ok: true,
        text: `从 ${file.name} 读到 ${r.points.length} 个点${arcs ? `、${arcs} 段圆弧` : ""}${r.closed ? "，闭合" : ""}${r.note ? `。${r.note}` : ""}`,
      });
    } catch (e) {
      if(mounted.current&&serial===importSerial.current)setNotice({ ok: false, text: String(e) });
    }
    finally{if(mounted.current&&serial===importSerial.current)setImporting(false);}
  };

  const save = async () => {
    if(pendingSave.current||importing||previewDoc!==doc||previewError)return;
    pendingSave.current=true;setSaving(true);setNotice(null);
    try {
      if (saveCandidate) {
        const saved=await saveCandidate(doc);
        if(mounted.current)setNotice(saved?{ok:true,text:"候选配置已保存，生产版本保持不变"}:{ok:false,text:"候选配置未保存，请修正错误后重试"});
        return;
      }
      const r = await recipeApi.save(doc, originalId);
      if(!mounted.current)return;
      setNotice({ ok: true, text: `已保存 ${r.id} v${r.version}` });
      onSaved(r.id);
    } catch (e) {
      if(mounted.current)setNotice({ ok: false, text: String(e) });
    }
    finally{pendingSave.current=false;if(mounted.current)setSaving(false);}
  };

  const summary = useMemo(() => {
    if (!preview||previewDoc!==doc) return "";
    const len = preview.segments.at(-1)?.s1 ?? 0;
    return `${preview.segments.length} 段 · 全长 ${len.toFixed(1)} mm · ${preview.points.x.length} 个测量点 · ${preview.closed ? "闭合" : "开放"}`;
  }, [preview,previewDoc,doc]);
  const currentPreview=previewDoc===doc?preview:null;

  return (
    <div className="rcp-editor">
      <div className="rcp-form">
        <div className="panel-toolbar">
          <h3 className="panel-title">
            {originalId ? `编辑 ${originalId}` : "新配方"} · 飞拍
            {originalId && <span className="muted mono"> v{doc.version}</span>}
          </h3>
          <button className="btn primary" onClick={()=>void save()} disabled={saving||importing||!!previewError||previewDoc!==doc}>
            <Save size={15} />
            {saving?"保存中…":saveCandidate ? "保存候选配置" : "保存"}
          </button>
        </div>
        {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`}>{notice.text}</div>}
        <fieldset disabled={saving} style={{border:0,padding:0,margin:0,minWidth:0,display:"flex",flexDirection:"column",gap:14}}>
        <Section title="基本">
          <div className="form-grid">
            <label className="field">
              <span>配方编号</span>
              <input className="input mono" value={doc.id} disabled={!!saveCandidate} onChange={(e) => set("id", e.target.value.trim())} />
            </label>
            <label className="field">
              <span>名称</span>
              <input className="input" value={doc.name} onChange={(e) => set("name", e.target.value)} />
            </label>
            <Num label="产品代码（PLC 下发）" value={doc.productCode} step={1} onChange={(v) => set("productCode", v)} />
            <Num label="测量点间距（mm）" value={doc.spacing} onChange={(v) => set("spacing", v)} />
            <Num label="中值滤波窗口（点，奇数）" value={doc.filterWindow} step={2} onChange={(v) => set("filterWindow", v)} />
            <Num label="允许断胶长度（mm）" value={doc.maxGapLen} onChange={(v) => set("maxGapLen", v)} />
          </div>
        </Section>

        <Section
          title="胶路"
          extra={
            <div className="segmented">
              <button className={doc.path.kind === "roundedRect" ? "active" : ""} onClick={() => setPath({ kind: "roundedRect", width: 240, height: 140, radius: 20 })}>
                圆角矩形
              </button>
              <button
                className={doc.path.kind === "polyline" ? "active" : ""}
                onClick={() => {
                  const p = parsePathText(polyText);
                  setPath({ kind: "polyline", points: p.points.length >= 2 ? p.points : [[0, 0], [100, 0]], bulges: p.points.length >= 2 ? p.bulges : [], closed: false, radius: 0 });
                }}
              >
                折线 / 导入
              </button>
            </div>
          }
        >
          {doc.path.kind === "roundedRect" ? (
            <div className="form-grid">
              <Num label="宽（mm）" value={doc.path.width} step={1} onChange={(v) => doc.path.kind === "roundedRect" && setPath({ ...doc.path, width: v })} />
              <Num label="高（mm）" value={doc.path.height} step={1} onChange={(v) => doc.path.kind === "roundedRect" && setPath({ ...doc.path, height: v })} />
              <Num label="圆角半径（mm）" value={doc.path.radius} step={1} onChange={(v) => doc.path.kind === "roundedRect" && setPath({ ...doc.path, radius: v })} />
            </div>
          ) : (
            <div className="rcp-poly">
              <textarea
                className="input mono"
                rows={7}
                aria-label="胶路点坐标"
                value={polyText}
                placeholder={"每行一个点：x, y（mm），可选第 3 列 bulge"}
                onChange={(e) => {
                  setPolyText(e.target.value);
                  if (doc.path.kind === "polyline") setPath({ ...doc.path, ...parsePathText(e.target.value) });
                }}
              />
              <div className="rcp-poly-side">
                <label className="btn">
                  <Upload size={15} />
                  {importing?"导入中…":"导入 CSV / DXF"}
                  <input aria-label="导入胶路文件" type="file" accept=".csv,.txt,.dxf" hidden onChange={(e) => {const file=e.target.files?.[0];e.target.value="";if(file)void importFile(file);}} />
                </label>
                <label className="check">
                  <input type="checkbox" checked={doc.path.closed} onChange={(e) => doc.path.kind === "polyline" && setPath({ ...doc.path, closed: e.target.checked })} />
                  闭合胶路
                </label>
                <Num label="拐角倒圆半径（mm）" value={doc.path.radius} onChange={(v) => doc.path.kind === "polyline" && setPath({ ...doc.path, radius: v })} />
                <p className="muted hint">
                  DXF 读 ENTITIES 段里的 LWPOLYLINE / POLYLINE（含圆弧与闭合）、CIRCLE，以及首尾相连的 LINE / ARC；有多条路径时取最长的。CSV 与上面的文本每行 x, y，第 3 列起默认不认；表头写成 x, y, bulge 时第 3 列是圆弧参数 bulge = tan(圆心角/4)，正值逆时针。导入文件时首尾重合的点当作闭合。
                </p>
              </div>
            </div>
          )}
        </Section>

        <Section title="判定限值（mm）">
          <div className="table-wrap">
            <table className="table rcp-limits">
              <thead>
                <tr>
                  <th />
                  {paramFields.map(([k, l]) => (
                    <th key={k}>{l}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                <LimitsTable label="直线段" value={doc.line} onChange={(v) => set("line", v)} widthDefault={widthDefault} />
                <LimitsTable label="拐角" value={doc.corner} onChange={(v) => set("corner", v)} widthDefault={widthDefault} />
              </tbody>
            </table>
          </div>
          <p className="muted hint">
            位置：内边到胶中线的距离。连续超出公差带超过"允许超差长度"判 NG，超出绝对限直接 NG。
          </p>
        </Section>

        <Section title="飞拍拍照点">
          <div className="form-grid">
            <label className="field">
              <span>触发方式</span>
              <select className="input" value={doc.triggerMode} onChange={(e) => set("triggerMode", e.target.value as RecipeDoc["triggerMode"])}>
                <option value="fly">飞拍（位置比较触发）</option>
                <option value="stop">停稳拍</option>
              </select>
            </label>
            <Num label="视野宽（mm）" value={doc.fov[0]} step={1} hint="拍照点没单独设视野时用" onChange={(v) => set("fov", [v, doc.fov[1]])} />
            <Num label="视野高（mm）" value={doc.fov[1]} step={1} hint="拍照点没单独设视野时用" onChange={(v) => set("fov", [doc.fov[0], v])} />
          </div>
          <ShotTable shots={doc.shots} fov={doc.fov} cameras={cameras} path={doc.path} onChange={(shots) => set("shots", shots)} />
        </Section>
        </fieldset>
      </div>

      <div className="rcp-preview">
        <div className="panel-head">
          <h4 className="sub-title">预览</h4>
          <span className="muted">{previewError ? "" : currentPreview?summary:"正在更新预览…"}</span>
        </div>
        {previewError && <div className="notice error">{previewError}</div>}
        {currentPreview && <TrajectoryMap layout={currentPreview} vis={new Array(currentPreview.points.x.length).fill("none")} className="traj rcp-traj" />}
        {currentPreview && (
          <div className="rcp-segs">
            {currentPreview.segments.map((g) => (
              <span key={g.name} className={g.kind === "corner" ? "corner" : ""}>
                {g.name} <b className="mono">{(g.s1 - g.s0).toFixed(1)}</b>
              </span>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
