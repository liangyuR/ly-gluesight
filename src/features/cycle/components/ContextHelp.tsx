import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { CircleHelp } from "lucide-react";
import "./ContextHelp.css";

export default function ContextHelp({ label, children }: { label: string; children: ReactNode }) {
  const id = useId();
  const trigger = useRef<HTMLButtonElement>(null);
  const bubble = useRef<HTMLDivElement>(null);
  const timeout = useRef<ReturnType<typeof setTimeout> | null>(null);
  const ignoreFocus = useRef(false);
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [position, setPosition] = useState({ left: 8, top: 8 });
  const clearClose = () => { if (timeout.current) clearTimeout(timeout.current); };
  const show = () => { clearClose(); setOpen(true); };
  const leave = () => {
    clearClose();
    timeout.current = setTimeout(() => {
      if (!pinned && document.activeElement !== trigger.current && !bubble.current?.contains(document.activeElement)) setOpen(false);
    }, 120);
  };
  const close = (restoreFocus = false) => {
    clearClose();
    setOpen(false);
    setPinned(false);
    if (restoreFocus && document.activeElement !== trigger.current) {
      ignoreFocus.current = true;
      trigger.current?.focus();
      ignoreFocus.current = false;
    }
  };
  useEffect(() => () => clearClose(), []);
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = trigger.current?.getBoundingClientRect();
      const popup = bubble.current?.getBoundingClientRect();
      if (!anchor || !popup) return;
      const left = Math.max(8, Math.min(anchor.left, window.innerWidth - popup.width - 8));
      const below = anchor.bottom + 6;
      const preferredTop = below + popup.height <= window.innerHeight - 8 ? below : anchor.top - popup.height - 6;
      const top = Math.max(8, Math.min(preferredTop, window.innerHeight - popup.height - 8));
      setPosition({ left, top });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    const observer = new ResizeObserver(place);
    if (bubble.current) observer.observe(bubble.current);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      observer.disconnect();
    };
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: globalThis.PointerEvent) => {
      if (!trigger.current?.contains(event.target as Node) && !bubble.current?.contains(event.target as Node)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(true); }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape, true);
    };
  }, [open]);
  return <>
    <button ref={trigger} type="button" className="context-help-trigger" aria-label={label} aria-expanded={open} aria-describedby={open ? id : undefined}
      onMouseEnter={show} onMouseLeave={leave} onFocus={() => { if (!ignoreFocus.current) show(); }} onBlur={leave}
      onClick={() => { clearClose(); if (pinned) close(); else { setOpen(true); setPinned(true); } }}><CircleHelp size={15} /></button>
    {open && createPortal(<div ref={bubble} id={id} role="tooltip" className="context-help-bubble" style={position} onMouseEnter={clearClose} onMouseLeave={leave} onFocus={clearClose} onBlur={leave}>{children}</div>, document.body)}
  </>;
}
