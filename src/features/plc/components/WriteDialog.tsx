import { useEffect, useRef, useState } from "react";
import Modal from "./Modal";
import { dataTypeLabels, formatValue } from "../meta";
import type { PlcPoint, PointValue } from "../types";

interface WriteDialogProps {
  point: PlcPoint;
  current: PointValue | undefined;
  blockedReason?: string;
  onWrite: (value: unknown) => Promise<void>;
  onClose: () => void;
}

export default function WriteDialog({ point, current, blockedReason = "", onWrite, onClose }: WriteDialogProps) {
  const [text, setText] = useState(current?.value !== null && current?.value !== undefined ? String(Number(current.value)) : "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const writing = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const isBool = point.dataType === "bool";

  const write = async (value: unknown) => {
    if (writing.current) return;
    if (blockedReason) return setError(blockedReason);
    writing.current = true;
    setBusy(true);
    setError("");
    try {
      await onWrite(value);
      if (mounted.current) onClose();
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      writing.current = false;
      if (mounted.current) setBusy(false);
    }
  };

  const submitNumber = () => {
    const n = Number(text);
    if (text.trim() === "" || !Number.isFinite(n)) return setError("请输入有效数值");
    write(n);
  };

  return (
    <Modal
      title={`写入 · ${point.name}`}
      onClose={onClose}
      closeDisabled={busy}
      width={420}
      footer={
        <>
          {error && <span className="form-error">{error}</span>}
          <button className="btn" disabled={busy} onClick={onClose}>
            取消
          </button>
          {!isBool && (
            <button className="btn primary" disabled={busy || !!blockedReason} onClick={submitNumber}>
              写入
            </button>
          )}
        </>
      }
    >
      {blockedReason && <p className="notice error" role="status">{blockedReason}</p>}
      <dl className="kv">
        <dt>地址</dt>
        <dd className="mono">{point.address}</dd>
        <dt>类型</dt>
        <dd>{dataTypeLabels[point.dataType]}</dd>
        <dt>当前值</dt>
        <dd className="mono">{formatValue(current)}</dd>
      </dl>
      <div className="write-input">
        {isBool ? (
          <div className="row">
            <button className="btn primary" disabled={busy || !!blockedReason} onClick={() => write(true)}>
              置 1 (ON)
            </button>
            <button className="btn" disabled={busy || !!blockedReason} onClick={() => write(false)}>
              置 0 (OFF)
            </button>
          </div>
        ) : (
          <input
            className="input mono"
            autoFocus
            disabled={busy || !!blockedReason}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submitNumber()}
          />
        )}
      </div>
    </Modal>
  );
}
