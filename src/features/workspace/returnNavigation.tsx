import { useLocation, useNavigate, type Location } from "react-router-dom";

/** 从工作台跳到相机、工位标定、PLC 等页面时带上的返回上下文。 */
export interface ReturnTarget { pathname: string; search: string; label: string; state?: unknown }
export interface ReturnNavigationState { returnTo?: ReturnTarget }

export function returnState<T extends object>(location: Pick<Location, "pathname" | "search">, label: string, state?: T): ReturnNavigationState & Partial<T> {
  return { ...state, returnTo: { pathname: location.pathname, search: location.search, label, state } } as ReturnNavigationState & Partial<T>;
}

export function ReturnButton({ disabled, hint }: { disabled?: boolean; hint?: string }) {
  const location = useLocation(), navigate = useNavigate();
  const target = (location.state as ReturnNavigationState | null)?.returnTo;
  if (!target) return null;
  return <div className="dev-bar"><button className="btn primary" disabled={disabled} onClick={() => navigate({ pathname: target.pathname, search: target.search }, { state: target.state })}>返回{target.label}</button>{hint && <span className="muted">{hint}</span>}</div>;
}
