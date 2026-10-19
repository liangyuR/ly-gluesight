import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, X } from "lucide-react";

export type SnackTone = "ok" | "info" | "warn";
/** 瞬时操作结果；错误与需要用户处理的提示仍放在页面内。 */
export type SnackMessage = string | { text: string; tone?: SnackTone };
interface Item { id: number; text: string; tone: SnackTone; leaving: boolean }
type Show = (message: SnackMessage) => void;

const ShowContext = createContext<Show | null>(null);
const ItemsContext = createContext<{ items: Item[]; dismiss: (id: number) => void } | null>(null);
const icons = { ok: CheckCircle2, info: Info, warn: AlertTriangle };

function motionMs(name: string) {
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return 0;
  return parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name)) || 0;
}

/** 与 qing-steel QingSteel.snackDuration 一致：4–10 s，随文字变长。 */
export function snackDuration(text: string) {
  return Math.min(10000, Math.max(4000, 3000 + text.length * 70));
}

export function SnackbarProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Item[]>([]);
  const serial = useRef(0);
  const removals = useRef<number[]>([]);
  useEffect(() => () => removals.current.forEach(clearTimeout), []);
  const retire = useCallback((ids: number[]) => {
    if (!ids.length) return;
    setItems(list => list.map(item => ids.includes(item.id) ? { ...item, leaving: true } : item));
    removals.current.push(window.setTimeout(() => setItems(list => list.filter(item => !ids.includes(item.id))), motionMs("--dur-short")));
  }, []);
  const itemsRef = useRef(items);
  itemsRef.current = items;
  const dismiss = useCallback((id: number) => retire([id]), [retire]);
  const show = useCallback((message: SnackMessage) => {
    const { text, tone = "ok" } = typeof message === "string" ? { text: message } : message;
    if (!text) return;
    retire(itemsRef.current.filter(item => !item.leaving).map(item => item.id));
    const id = ++serial.current;
    setItems(list => [...list, { id, text, tone, leaving: false }]);
  }, [retire]);
  return <ShowContext.Provider value={show}><ItemsContext.Provider value={{ items, dismiss }}>{children}</ItemsContext.Provider></ShowContext.Provider>;
}

function Snackbar({ item, onDismiss }: { item: Item; onDismiss: (id: number) => void }) {
  const remaining = useRef(snackDuration(item.text));
  const timer = useRef(0);
  const started = useRef(0);
  const held = useRef({ hover: false, focus: false });
  const arm = () => {
    if (timer.current || item.leaving || held.current.hover || held.current.focus) return;
    started.current = Date.now();
    timer.current = window.setTimeout(() => { timer.current = 0; onDismiss(item.id); }, remaining.current);
  };
  const pause = () => {
    if (!timer.current) return;
    clearTimeout(timer.current);
    timer.current = 0;
    remaining.current = Math.max(1500, remaining.current - (Date.now() - started.current));
  };
  useEffect(() => { arm(); return () => { clearTimeout(timer.current); timer.current = 0; }; }, []);
  useEffect(() => { if (item.leaving) { clearTimeout(timer.current); timer.current = 0; } }, [item.leaving]);
  const Icon = icons[item.tone];
  return <div className={"qs-snackbar " + item.tone + (item.leaving ? " out" : "")}
    onMouseEnter={() => { held.current.hover = true; pause(); }} onMouseLeave={() => { held.current.hover = false; arm(); }}
    onFocus={() => { held.current.focus = true; pause(); }} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) { held.current.focus = false; arm(); } }}>
    <Icon aria-hidden="true" />
    <span className="msg">{item.text}</span>
    <button type="button" className="qs-btn icon" aria-label="关闭提示" onClick={() => onDismiss(item.id)}><X size={18} aria-hidden="true" /></button>
  </div>;
}

/** 全局提示层：放在主区顶部，浮在页面之上，不占布局。 */
export function SnackbarHost() {
  const channel = useContext(ItemsContext);
  return <div className="snackbar-anchor">
    <div className="qs-snackbars top" role="status" aria-live="polite">
      {channel?.items.map(item => <Snackbar key={item.id} item={item} onDismiss={channel.dismiss} />)}
    </div>
  </div>;
}

/** 没有挂载 SnackbarProvider 时提示不显示（单元测试按需挂载）。 */
export function useSnackbar() {
  return useContext(ShowContext) ?? ignore;
}
function ignore() {}
