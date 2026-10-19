import { useLocation, useNavigate, type Location } from "react-router-dom";
import { Button } from "qing-steel";

/** 从工作台跳到相机、工位标定、PLC 等页面时带上的返回上下文。 */
export interface ReturnTarget { pathname: string; search: string; label: string; state?: unknown }
export interface ReturnNavigationState { returnTo?: ReturnTarget }

export function returnState<T extends object>(location: Pick<Location, "pathname" | "search">, label: string, state?: T): ReturnNavigationState & Partial<T> {
  return { ...state, returnTo: { pathname: location.pathname, search: location.search, label, state } } as ReturnNavigationState & Partial<T>;
}

export function ReturnButton({ disabled, hint, guard }: { disabled?: boolean; hint?: string; guard?: (go: () => void) => void }) {
  const location = useLocation(), navigate = useNavigate();
  const target = (location.state as ReturnNavigationState | null)?.returnTo;
  if (!target) return null;
  const go = () => navigate({ pathname: target.pathname, search: target.search }, { state: target.state });
  return <div className="dev-bar"><Button variant="filled" disabled={disabled} onClick={() => guard ? guard(go) : go()}>返回{target.label}</Button>{hint && <span className="muted">{hint}</span>}</div>;
}
