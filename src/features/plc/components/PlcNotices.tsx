import type { ReactNode } from "react";
import { Accordion, Alert, type AlertTone } from "qing-steel";

export interface PlcNotice {
  key: string;
  tone: AlertTone;
  text: ReactNode;
  action?: ReactNode;
  /** 只进“还有 n 条”，不作首条（例如页面顶部已有同义提示）。 */
  secondary?: boolean;
}

/** 同一时刻只显示一条提示，其余按同一优先级折叠到“还有 n 条提示”。 */
export default function PlcNotices({ notices }: { notices: PlcNotice[] }) {
  const first = notices.find(n => !n.secondary);
  const rest = notices.filter(n => n !== first);
  if (!first && rest.length === 0) return null;
  return <div className="plc-notices">
    {first && <Alert tone={first.tone} action={first.action}>{first.text}</Alert>}
    {rest.length > 0 && <Accordion collapsible items={[{
      value: "notices",
      title: first ? `还有 ${rest.length} 条提示` : `${rest.length} 条提示`,
      content: <div className="plc-notices-list">
        {rest.map(n => <Alert key={n.key} tone={n.tone} action={n.action}>{n.text}</Alert>)}
      </div>,
    }]} />}
  </div>;
}
