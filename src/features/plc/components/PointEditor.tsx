import { useEffect, useRef, useState } from "react";
import Modal from "./Modal";
import { plcApi } from "../api";
import { dataTypeLabels, edgeLabels, isMultiWord, protocols } from "../meta";
import type { Access, ConnectionConfig, DataType, EdgeMode, PlcPoint, TagPreset, WordOrder } from "../types";

interface PointEditorProps {
  initial: PlcPoint;
  isNew: boolean;
  existingIds: string[];
  connection: ConnectionConfig;
  tagPresets: TagPreset[];
  blockedReason?: string;
  onSave: (point: PlcPoint) => void;
  onClose: () => void;
}

export default function PointEditor({ initial, isNew, existingIds, connection, tagPresets, blockedReason = "", onSave, onClose }: PointEditorProps) {
  const [p, setP] = useState<PlcPoint>(initial);
  const [error, setError] = useState("");
  const [check, setCheck] = useState<{ ok: boolean; text: string } | null>(null);
  const [tagInput, setTagInput] = useState("");
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const mounted = useRef(true);
  const blocked = useRef(blockedReason);
  blocked.current = blockedReason;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const update = (patch: Partial<PlcPoint>) => {
    setError("");
    if (patch.address !== undefined || patch.dataType !== undefined) setCheck(null);
    setP((prev) => ({ ...prev, ...patch }));
  };
  const meta = protocols[connection.protocol];

  useEffect(() => {
    let alive = true;
    setCheck(null);
    if (!p.address.trim()) {
      setCheck(null);
      return;
    }
    const t = setTimeout(() => {
      plcApi
        .checkAddress(connection, p.address, p.dataType)
        .then((text) => { if (alive) setCheck({ ok: true, text }); })
        .catch((e) => { if (alive) setCheck({ ok: false, text: String(e) }); });
    }, 250);
    return () => { alive = false; clearTimeout(t); };
  }, [p.address, p.dataType, connection]);

  const toggleTag = (tag: string) =>
    update({ tags: p.tags.includes(tag) ? p.tags.filter((t) => t !== tag) : [...p.tags, tag] });

  const addTag = () => {
    const t = tagInput.trim();
    if (t && !p.tags.includes(t)) update({ tags: [...p.tags, t] });
    setTagInput("");
  };

  const submit = async () => {
    if (pending.current) return;
    if (blocked.current) return setError(blocked.current);
    const id = p.id.trim();
    if (!id) return setError("ID 不能为空");
    if (!p.name.trim()) return setError("名称不能为空");
    if (!p.address.trim()) return setError("地址不能为空");
    if (existingIds.includes(id) && (isNew || id !== initial.id)) return setError("ID 已存在");
    pending.current = true; setSaving(true); setError("");
    try {
      await plcApi.checkAddress(connection, p.address.trim(), p.dataType);
      if (mounted.current) {
        if (blocked.current) setError(blocked.current);
        else onSave({ ...p, id, name: p.name.trim(), address: p.address.trim() });
      }
    } catch (e) { if (mounted.current) setError(String(e)); }
    finally { pending.current = false; if (mounted.current) setSaving(false); }
  };

  const presetValues = new Set(tagPresets.map((t) => t.value));
  const customTags = p.tags.filter((t) => !presetValues.has(t));

  return (
    <Modal
      title={isNew ? "新增点位" : "编辑点位"}
      closeDisabled={saving}
      onClose={() => { if (!pending.current) onClose(); }}
      width={660}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" disabled={saving} onClick={onClose}>
            取消
          </button>
          <button className="btn primary" disabled={saving || !!blockedReason} onClick={() => void submit()}>
            确定
          </button>
        </>
      }
    >
      {blockedReason && <p className="notice error" role="status">{blockedReason}</p>}
      <fieldset className="form-grid two" disabled={saving || !!blockedReason} style={{ border: 0, padding: 0, margin: 0 }}>
        <label className="field">
          <span>名称</span>
          <input className="input" value={p.name} autoFocus onChange={(e) => update({ name: e.target.value })} />
        </label>
        <label className="field">
          <span>ID</span>
          <input className="input mono" value={p.id} onChange={(e) => update({ id: e.target.value })} />
        </label>
        <label className="field span-2">
          <span>地址（{meta.label}）</span>
          <input
            className="input mono"
            placeholder={meta.examples.slice(0, 4).join("  ")}
            value={p.address}
            onChange={(e) => update({ address: e.target.value })}
          />
          <span className={`addr-check ${check ? (check.ok ? "ok" : "ng") : "muted"}`}>
            {check ? check.text : meta.hint}
          </span>
        </label>
        <label className="field">
          <span>数据类型</span>
          <select className="input" value={p.dataType} onChange={(e) => update({ dataType: e.target.value as DataType, wordOrder: e.target.value === "bool" ? null : p.wordOrder })}>
            {Object.entries(dataTypeLabels).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>字节序</span>
          <select
            className="input"
            value={p.wordOrder ?? ""}
            disabled={!isMultiWord(p.dataType) && p.dataType !== "u16" && p.dataType !== "i16"}
            onChange={(e) => update({ wordOrder: (e.target.value || null) as WordOrder | null })}
          >
            <option value="">协议默认（{meta.defaultOrder}）</option>
            <option value="ABCD">ABCD 大端</option>
            <option value="CDAB">CDAB 字交换</option>
            <option value="BADC">BADC 字节交换</option>
            <option value="DCBA">DCBA 小端</option>
          </select>
        </label>
        <label className="field">
          <span>读写</span>
          <select className="input" value={p.access} onChange={(e) => update({ access: e.target.value as Access })}>
            <option value="read">只读</option>
            <option value="readWrite">读写</option>
          </select>
        </label>
        <label className="field">
          <span>边沿事件</span>
          <select className="input" value={p.edge} onChange={(e) => update({ edge: e.target.value as EdgeMode })}>
            {Object.entries(edgeLabels).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </label>
        <div className="field span-2">
          <span>标签</span>
          <div className="tag-editor">
            {tagPresets.map((t) => (
              <button key={t.value} type="button" className={`chip ${p.tags.includes(t.value) ? "on" : ""}`} onClick={() => toggleTag(t.value)}>
                {t.label}
              </button>
            ))}
            {customTags.map((t) => (
              <button key={t} type="button" className="chip on" onClick={() => toggleTag(t)} title="点击移除">
                {t} ×
              </button>
            ))}
            <input
              className="input tag-input"
              placeholder="自定义标签，回车添加"
              value={tagInput}
              onChange={(e) => setTagInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addTag();
                }
              }}
            />
          </div>
        </div>
        <label className="field span-2">
          <span>描述</span>
          <input className="input" value={p.description} onChange={(e) => update({ description: e.target.value })} />
        </label>
        <label className="check span-2">
          <input type="checkbox" checked={p.logChanges} onChange={(e) => update({ logChanges: e.target.checked })} />
          <span>记录值变化到 PLC 日志（高频变化的点位建议关闭）</span>
        </label>
      </fieldset>
    </Modal>
  );
}
