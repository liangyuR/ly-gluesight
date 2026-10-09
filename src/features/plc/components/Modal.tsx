import { useEffect, useId, useRef, type ReactNode } from "react";
import { X } from "lucide-react";

interface ModalProps {
  title: string;
  onClose: () => void;
  closeDisabled?: boolean;
  footer?: ReactNode;
  width?: number;
  children: ReactNode;
}

export default function Modal({ title, onClose, closeDisabled = false, footer, width = 520, children }: ModalProps) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = () => { if (!closeDisabled) onClose(); };
  const titleId = useId();
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const controls = () => Array.from(ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex="0"]') ?? []);
    const frame = requestAnimationFrame(() => (ref.current?.querySelector<HTMLElement>(".modal-body input:not(:disabled)") ?? controls()[0] ?? ref.current)?.focus());
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); close.current(); }
      if (event.key !== "Tab") return;
      const items = controls();
      if (!items.length) { event.preventDefault(); ref.current?.focus(); return; }
      const index = items.indexOf(document.activeElement as HTMLElement);
      event.preventDefault();
      items[index < 0 ? (event.shiftKey ? items.length-1 : 0) : (index + (event.shiftKey ? -1 : 1) + items.length) % items.length].focus();
    };
    document.addEventListener("keydown",key);
    return () => { cancelAnimationFrame(frame); document.removeEventListener("keydown",key); if (previous?.isConnected) previous.focus(); };
  },[]);
  return (
    <div className="modal-mask" onMouseDown={(e) => e.target === e.currentTarget && close.current()}>
      <div ref={ref} className="modal" style={{ width, maxWidth:"calc(100vw - 40px)" }} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}>
        <div className="modal-header">
          <h3 id={titleId}>{title}</h3>
          <button className="icon-btn" disabled={closeDisabled} onClick={() => close.current()} aria-label="关闭">
            <X size={18} />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>
  );
}
