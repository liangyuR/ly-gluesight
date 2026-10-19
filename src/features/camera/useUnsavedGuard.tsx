import { useEffect, useRef, useState } from "react";
import { Button } from "qing-steel";
import Modal from "../plc/components/Modal";

/** 有未保存修改时，切换相机、点链接离开、返回前先确认：保存并应用 / 放弃 / 取消。 */
export function useUnsavedGuard({ dirty, save, discard }: { dirty: boolean; save: () => Promise<boolean>; discard: () => void }) {
  const [pending, setPending] = useState<(() => void) | null>(null);
  const [saving, setSaving] = useState(false);
  const bypass = useRef(false);
  const dirtyRef = useRef(dirty); dirtyRef.current = dirty;

  useEffect(() => {
    const click = (event: MouseEvent) => {
      if (bypass.current || !dirtyRef.current || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const anchor = (event.target as Element | null)?.closest?.("a[href]");
      if (!(anchor instanceof HTMLAnchorElement) || anchor.target === "_blank") return;
      event.preventDefault();
      event.stopPropagation();
      setPending(() => () => { bypass.current = true; try { anchor.click(); } finally { bypass.current = false; } });
    };
    document.addEventListener("click", click, true);
    return () => document.removeEventListener("click", click, true);
  }, []);

  const guard = (action: () => void) => { if (dirtyRef.current) setPending(() => action); else action(); };
  const close = () => { if (!saving) setPending(null); };
  const proceed = (action: () => void) => { setPending(null); action(); };

  const dialog = pending && <Modal title="有未保存的相机参数" onClose={close} closeDisabled={saving} width={420} footer={<>
    <Button disabled={saving} onClick={close}>取消</Button>
    <Button disabled={saving} onClick={() => { discard(); proceed(pending); }}>放弃修改</Button>
    <Button variant="filled" disabled={saving} onClick={async () => { setSaving(true); const ok = await save().finally(() => setSaving(false)); if (ok) proceed(pending); else setPending(null); }}>{saving ? "写入中…" : "保存并应用"}</Button>
  </>}><p>当前相机的参数修改还没有写入相机。</p></Modal>;

  return { guard, dialog };
}
