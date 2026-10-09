import { useEffect, useRef, useState } from "react";
import Modal from "./Modal";
import { dataTypeLabels, edgeLabels, newPoint } from "../meta";
import type { PlcPoint } from "../types";

interface PointsJsonDialogProps {
  points: PlcPoint[];
  onApply: (points: PlcPoint[]) => void;
  onClose: () => void;
}

export default function PointsJsonDialog({ points, onApply, onClose }: PointsJsonDialogProps) {
  const [text, setText] = useState(() => JSON.stringify(points, null, 2));
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [copying, setCopying] = useState(false);
  const copyPending = useRef(false);
  const alive = useRef(true);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => { alive.current = true; return () => { alive.current = false; clearTimeout(copyTimer.current); }; }, []);

  const apply = () => {
    try {
      const parsed = JSON.parse(text);
      if (!Array.isArray(parsed)) throw new Error("需要点位数组");
      const ids = new Set<string>();
      const next = parsed.map((raw, index) => {
        const fail = (field: string): never => { throw new Error(`第 ${index + 1} 项的${field}无效`); };
        if (!raw || typeof raw !== "object" || Array.isArray(raw)) fail("点位对象");
        if (typeof raw.id !== "string" || !raw.id.trim()) fail("id");
        const p: PlcPoint = { ...newPoint(), ...raw };
        for (const field of ["id", "name", "address"] as const) {
          if (typeof p[field] !== "string" || !p[field].trim()) fail(field);
          p[field] = p[field].trim();
        }
        if (ids.has(p.id)) throw new Error(`点位 ID 重复：${p.id}`);
        ids.add(p.id);
        if (!Object.prototype.hasOwnProperty.call(dataTypeLabels, p.dataType)) fail("数据类型");
        if (!["read", "readWrite"].includes(p.access)) fail("读写方式");
        if (!Object.prototype.hasOwnProperty.call(edgeLabels, p.edge)) fail("边沿事件");
        if (p.wordOrder !== null && !["ABCD", "CDAB", "BADC", "DCBA"].includes(p.wordOrder)) fail("字节序");
        if (typeof p.logChanges !== "boolean") fail("变化日志开关");
        if (typeof p.description !== "string") fail("说明");
        if (!Array.isArray(p.tags) || p.tags.some(tag => typeof tag !== "string")) fail("标签");
        p.tags = [...new Set(p.tags.map(tag => tag.trim()).filter(Boolean))];
        if (p.dataType === "bool") p.wordOrder = null;
        return p;
      });
      onApply(next);
      onClose();
    } catch (e) {
      setError(`解析失败：${e instanceof Error ? e.message : String(e)}`);
    }
  };

  const copy = async () => {
    if (copyPending.current) return;
    copyPending.current = true; setCopying(true); setError("");
    try {
      await navigator.clipboard.writeText(text);
      if (alive.current) {
        setCopied(true); clearTimeout(copyTimer.current);
        copyTimer.current = setTimeout(() => setCopied(false), 1500);
      }
    } catch (e) { if (alive.current) setError(`复制失败：${String(e)}`); }
    finally { copyPending.current = false; if (alive.current) setCopying(false); }
  };

  return (
    <Modal
      title="导入 / 导出地址表"
      onClose={onClose}
      width={720}
      footer={
        <>
          {error && <span className="form-error" role="alert">{error}</span>}
          <button className="btn" disabled={copying} onClick={copy}>
            {copied ? "已复制" : "复制"}
          </button>
          <button className="btn" onClick={onClose}>
            取消
          </button>
          <button className="btn primary" onClick={apply}>
            应用到地址表
          </button>
        </>
      }
    >
      <p className="muted hint">复制下方 JSON 可备份或在设备间共享；粘贴 JSON 后点击应用将替换当前地址表（需再保存配置）。</p>
      <textarea className="input code" aria-label="地址表 JSON" spellCheck={false} value={text} onChange={(e) => { setText(e.target.value); setError(""); setCopied(false); }} />
    </Modal>
  );
}
