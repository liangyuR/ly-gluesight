import type { ReactNode } from "react";
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
  return (
    <Dialog open title={title} footer={footer} closeDisabled={closeDisabled} width={width} initialFocus="first-field" className={className} onOpenChange={open => { if (!open) onClose(); }}>
      {children}
    </Dialog>
  );
}
