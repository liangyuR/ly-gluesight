import { useEffect, useRef, useState } from "react";
import { Alert, Button, Stack, TextField } from "qing-steel";
import Modal from "./Modal";
import { dataTypeLabels, formatValue, isText } from "../meta";
import type { PlcPoint, PointValue } from "../types";

interface WriteDialogProps {
  point: PlcPoint;
  current: PointValue | undefined;
  blockedReason?: string;
  onWrite: (value: unknown) => Promise<void>;
  onClose: () => void;
}

export default function WriteDialog({ point, current, blockedReason = "", onWrite, onClose }: WriteDialogProps) {
  const [text, setText] = useState(current?.value !== null && current?.value !== undefined ? Array.isArray(current.value) ? formatValue(current) : typeof current.value === "string" ? current.value : String(Number(current.value)) : "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const writing = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const isBool = point.dataType === "bool";
  const isString = isText(point.dataType);

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

  const submitValue = () => {
    if (isString) return write(text);
    if (point.dataType === "bytes") {
      const bytes = text.trim().split(/\s+/);
      if (!text.trim() || bytes.some(byte => !/^[\da-f]{2}$/i.test(byte))) return setError("请输入空格分隔的两位十六进制字节，如 00 01 FF");
      return write(bytes.map(byte => parseInt(byte, 16)));
    }
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
          <Button disabled={busy} onClick={onClose}>取消</Button>
          {!isBool && <Button variant="filled" loading={busy} disabled={!!blockedReason} onClick={submitValue}>写入</Button>}
        </>
      }
    >
      {blockedReason && <Alert tone="warn">{blockedReason}</Alert>}
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
          <Stack>
            <Button variant="filled" disabled={busy || !!blockedReason} onClick={() => write(true)}>置 1 (ON)</Button>
            <Button disabled={busy || !!blockedReason} onClick={() => write(false)}>置 0 (OFF)</Button>
          </Stack>
        ) : (
          <TextField
            aria-label="写入值"
            disabled={busy || !!blockedReason}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submitValue()}
          />
        )}
      </div>
      {error && <Alert tone="ng">{error}</Alert>}
    </Modal>
  );
}
