import { useLayoutEffect, useRef, useState, type HTMLAttributes, type ReactNode } from "react";

type Placement = { x: number; y: number; width: number; height: number };
type Layout = { width: number; height: number; items: Placement[] };
type Handoff = { placement: Placement; layout: Layout };
const handoffs = new Map<string, Handoff>();

function bounds(element: HTMLElement, container: HTMLElement): Placement {
  const parent = container.getBoundingClientRect();
  const rect = element.getBoundingClientRect();
  return { x: rect.left - parent.left - container.clientLeft + container.scrollLeft, y: rect.top - parent.top - container.clientTop + container.scrollTop, width: rect.width, height: rect.height };
}

function samePlacement(a: Placement, b: Placement) {
  return Math.abs(a.x - b.x) < 0.5 && Math.abs(a.y - b.y) < 0.5 && Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5;
}

function sameLayout(a: Layout, b: Layout) {
  return Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5 && a.items.length === b.items.length && a.items.every((item, i) => samePlacement(item, b.items[i]));
}

const frame = (position: Placement) => ({ transform: `translate(${position.x}px, ${position.y}px)`, width: `${position.width}px`, height: `${position.height}px` });

export default function SelectionTrack({ as: Tag = "div", activeKey, trackId, className = "", children, ...props }: HTMLAttributes<HTMLElement> & { as?: "div" | "nav"; activeKey: string; trackId?: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement & HTMLElement>(null);
  const indicator = useRef<HTMLSpanElement>(null);
  const previousKey = useRef(activeKey);
  const currentPlacement = useRef<Placement | null>(null);
  const currentLayout = useRef<Layout | null>(null);
  const pending = useRef<{ from: Placement; to: Placement } | null>(null);
  const animation = useRef<Animation | null>(null);
  const [placement, setPlacement] = useState<Placement | null>(null);

  useLayoutEffect(() => {
    const container = ref.current;
    if (!container) return;
    const changed = previousKey.current !== activeKey;
    previousKey.current = activeKey;
    const inherited = trackId ? handoffs.get(trackId) : undefined;
    if (trackId) handoffs.delete(trackId);
    const measure = (animate = false, handoff?: Handoff) => {
      const active = container.querySelector<HTMLElement>(":scope > button.active, :scope > a[aria-current='page']");
      if (!active) { animation.current?.cancel(); currentPlacement.current = null; setPlacement(null); return; }
      const parent = container.getBoundingClientRect();
      const next = bounds(active, container);
      if (!next.width || !next.height) { currentPlacement.current = null; setPlacement(null); return; }
      const layout = { width: parent.width, height: parent.height, items: Array.from(container.children).filter((child): child is HTMLElement => child instanceof HTMLElement && child.matches("button, a")).map(child => bounds(child, container)) };
      const previous = currentPlacement.current;
      const unchanged = previous && samePlacement(previous, next);
      currentLayout.current = layout;
      if (unchanged && !handoff) return;
      const from = handoff && sameLayout(handoff.layout, layout) ? handoff.placement : animate && previous && indicator.current ? bounds(indicator.current, container) : null;
      animation.current?.cancel();
      pending.current = from && !samePlacement(from, next) && Math.abs(from.y - next.y) < 1 ? { from, to: next } : null;
      currentPlacement.current = next;
      setPlacement(next);
    };
    measure(changed, inherited);
    const resize = () => measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(resize);
    observer?.observe(container);
    for (const child of container.children) if (child.matches("button, a")) observer?.observe(child);
    window.addEventListener("resize", resize);
    return () => { observer?.disconnect(); window.removeEventListener("resize", resize); };
  }, [activeKey, trackId]);

  useLayoutEffect(() => {
    const move = pending.current;
    const element = indicator.current;
    if (!move || !placement || !element) return;
    pending.current = null;
    if (!element.animate || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    const tokens = getComputedStyle(element);
    const duration = tokens.getPropertyValue("--dur-medium").trim();
    const easing = tokens.getPropertyValue("--ease-standard").trim();
    const scale = `scale(${move.to.width ? move.from.width / move.to.width : 1}, ${move.to.height ? move.from.height / move.to.height : 1})`;
    animation.current = element.animate([{ transform: `translate(${move.from.x}px, ${move.from.y}px) ${scale}` }, { transform: `translate(${move.to.x}px, ${move.to.y}px)` }], { duration: parseFloat(duration) * (duration.endsWith("ms") ? 1 : 1000), easing });
  }, [placement]);

  useLayoutEffect(() => {
    const container = ref.current;
    return () => {
      if (trackId && container && currentPlacement.current && currentLayout.current) {
        const visible = pending.current?.from ?? (indicator.current ? bounds(indicator.current, container) : currentPlacement.current);
        const saved = { placement: visible.width ? visible : currentPlacement.current, layout: currentLayout.current };
        handoffs.set(trackId, saved);
        queueMicrotask(() => { if (handoffs.get(trackId) === saved) handoffs.delete(trackId); });
      }
      animation.current?.cancel();
    };
  }, [trackId]);

  return <Tag {...props} ref={ref} className={`${className} selection-track`} data-selection-ready={placement ? "true" : undefined}>
    {placement && <span ref={indicator} aria-hidden="true" className="selection-track-indicator" style={frame(placement)} />}
    {children}
  </Tag>;
}
