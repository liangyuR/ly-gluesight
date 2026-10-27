import { useCallback, useEffect, useRef, type ReactNode } from "react";
import { Dialog } from "qing-steel";

interface ModalProps {
  title: string;
  onClose: () => void;
  closeDisabled?: boolean;
  footer?: ReactNode;
  width?: number;
  children: ReactNode;
  className?: string;
}

export default function Modal({ title, onClose, closeDisabled = false, footer, width = 520, children, className = "" }: ModalProps) {
  const body = useRef<HTMLDivElement | null>(null);
  const attach = useCallback((node: HTMLDivElement | null) => {
    body.current = node;
    node?.closest<HTMLElement>(".qs-modal-content")?.style.setProperty("--modal-w", `${width}px`);
  }, [width]);
  useEffect(() => {
    const frame = requestAnimationFrame(() => body.current?.querySelector<HTMLElement>("input:not(:disabled)")?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);
  return (
    <Dialog open title={title} footer={footer} closeDisabled={closeDisabled} className={`gs-modal ${className}`} onOpenChange={open => { if (!open) onClose(); }}>
      <div ref={attach} className="modal-body">{children}</div>
    </Dialog>
  );
}
