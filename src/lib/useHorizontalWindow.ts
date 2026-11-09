import { useLayoutEffect, useRef, useState } from "react";

export function useHorizontalWindow<T extends HTMLElement>(count: number, step: number, selected: number | null, leading = 0) {
  const ref = useRef<T>(null);
  const [viewport, setViewport] = useState({ left: 0, width: 800 });
  const update = () => {
    const element = ref.current;
    if (element) setViewport(previous => {
      const next = { left: element.scrollLeft, width: element.clientWidth || 800 };
      return previous.left === next.left && previous.width === next.width ? previous : next;
    });
  };
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    update();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(element);
    window.addEventListener("resize", update);
    return () => { observer?.disconnect(); window.removeEventListener("resize", update); };
  }, [count]);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element || selected == null || selected < 0 || selected >= count) return;
    const left = leading + selected * step, right = left + step;
    const width = element.clientWidth || 800;
    if (left < element.scrollLeft) element.scrollLeft = left;
    else if (right > element.scrollLeft + width) element.scrollLeft = right - width;
    update();
  }, [selected, count, step, leading]);
  const start = Math.max(0, Math.min(count, Math.floor((viewport.left - leading) / step) - 6));
  const end = Math.min(count, Math.ceil((viewport.left + viewport.width - leading) / step) + 6);
  return { ref, start, end, onScroll: update };
}
