import { useEffect, useRef, useState } from "react";
import { Alert, Button, Checkbox, Chip, Grid, Select, Stack, TextField } from "qing-steel";
import Modal from "./Modal";
import { plcApi } from "../api";
import { dataTypeLabels, edgeLabels, isMultiWord, isText, s7Protocol } from "../meta";
import type { Access, ConnectionConfig, DataType, EdgeMode, PlcPoint, TagPreset, WordOrder } from "../types";

interface PointEditorProps {
  initial: PlcPoint;
  isNew: boolean;
  existingIds: string[];
  connection: ConnectionConfig;
  tagPresets: TagPreset[];
  blockedReason?: string;
  /** 握手点位由模板定义：只允许改描述和记录变化。 */
  templateLocked?: boolean;
  onSave: (point: PlcPoint) => void;
  onClose: () => void;
}

export const templateLockedHint = "握手点位由模板定义，调整地址请使用「重置 S7 通讯配置」";

export default function PointEditor({ initial, isNew, existingIds, connection, tagPresets, blockedReason = "", templateLocked = false, onSave, onClose }: PointEditorProps) {
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
  const meta = s7Protocol;

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
    if (templateLocked) return onSave({ ...initial, description: p.description, logChanges: p.logChanges });
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
          <Button disabled={saving} onClick={onClose}>取消</Button>
          <Button variant="filled" loading={saving} disabled={!!blockedReason} onClick={() => void submit()}>确定</Button>
        </>
      }
    >
      {blockedReason && <Alert tone="warn">{blockedReason}</Alert>}
      {templateLocked && <Alert tone="info">{templateLockedHint}；这里只能改描述和是否记录变化。</Alert>}
      <fieldset disabled={saving || !!blockedReason} style={{ border: 0, padding: 0, margin: 0 }}>
        <Grid minWidth={260} gap={14}>
          <fieldset disabled={templateLocked} title={templateLocked ? templateLockedHint : undefined} className="plc-wide plc-editor-locked">
          <Grid minWidth={260} gap={14}>
          <TextField label="名称" value={p.name} onChange={(e) => update({ name: e.target.value })} />
          <TextField label="ID" value={p.id} onChange={(e) => update({ id: e.target.value })} />
          <div className="plc-wide">
            <TextField
              label={`地址（${meta.label}）`}
              placeholder={meta.examples.slice(0, 4).join("  ")}
              value={p.address}
              onChange={(e) => update({ address: e.target.value })}
              error={check && !check.ok ? check.text : undefined}
              description={check?.ok ? <span className="addr-check ok">{check.text}</span>
                : <>{meta.hint}<br />示例：<span className="mono">{meta.examples.join("  ")}</span></>}
            />
          </div>
          <Select label="数据类型" value={p.dataType} onValueChange={(v) => update({ dataType: v as DataType, wordOrder: v === "bool" || isText(v as DataType) ? null : p.wordOrder })}
            options={Object.entries(dataTypeLabels).map(([k, v]) => ({ value: k, label: v }))} />
          <Select label="字节序" value={p.wordOrder ?? ""} disabled={!isMultiWord(p.dataType) && p.dataType !== "u16" && p.dataType !== "i16"}
            onValueChange={(v) => update({ wordOrder: (v || null) as WordOrder | null })}
            options={[
              { value: "", label: `协议默认（${meta.defaultOrder}）` },
              { value: "ABCD", label: "ABCD 大端" },
              { value: "CDAB", label: "CDAB 字交换" },
              { value: "BADC", label: "BADC 字节交换" },
              { value: "DCBA", label: "DCBA 小端" },
            ]} />
          <Select label="读写" value={p.access} onValueChange={(v) => update({ access: v as Access })}
            options={[{ value: "read", label: "只读" }, { value: "readWrite", label: "读写" }]} />
          <Select label="边沿事件" value={p.edge} onValueChange={(v) => update({ edge: v as EdgeMode })}
            options={Object.entries(edgeLabels).map(([k, v]) => ({ value: k, label: v }))} />
          <div className="plc-wide">
            <Stack direction="column" align="stretch" gap={6}>
              <span className="plc-group-label">标签</span>
              <Stack gap={6}>
                {tagPresets.map((t) => (
                  <Chip key={t.value} selected={p.tags.includes(t.value)} onClick={() => toggleTag(t.value)}>{t.label}</Chip>
                ))}
                {customTags.map((t) => (
                  <Chip key={t} onRemove={() => toggleTag(t)} removeLabel={`移除标签 ${t}`}>{t}</Chip>
                ))}
                <TextField
                  className="plc-tag-input"
                  aria-label="自定义标签"
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
              </Stack>
            </Stack>
          </div>
          </Grid>
          </fieldset>
          <div className="plc-wide">
            <TextField label="描述" value={p.description} onChange={(e) => update({ description: e.target.value })} />
          </div>
          <div className="plc-wide">
            <Checkbox label="记录值变化到 PLC 日志（高频变化的点位建议关闭）" checked={p.logChanges} onCheckedChange={(checked) => update({ logChanges: checked })} />
          </div>
        </Grid>
      </fieldset>
      {error && <Alert tone="ng">{error}</Alert>}
    </Modal>
  );
}
