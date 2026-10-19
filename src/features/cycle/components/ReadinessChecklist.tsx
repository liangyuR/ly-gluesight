import { Link } from "react-router-dom";
import { Card, StatusBadge } from "qing-steel";
import { devLyflowHint, targetLinks } from "../readiness";
import type { Readiness, ReadinessItem, ReadinessState, ReadinessTarget } from "../types";
import ContextHelp from "./ContextHelp";

const stateText: Record<ReadinessState, string> = { ok: "就绪", waiting: "等待", blocked: "阻塞" };
const stateTone = { ok: "ok", waiting: "warn", blocked: "ng" } as const satisfies Record<ReadinessState, string>;

export default function ReadinessChecklist({ readiness, embedded = false, includeReady = true }: { readiness: Readiness; embedded?: boolean; includeReady?: boolean }) {
  const visibleItems = embedded ? readiness.items.filter(item => item.state !== "ok") : readiness.items;
  const readyItems = readiness.items.filter(item => item.state === "ok");
  if (!embedded) return (
    <Card className="readiness" role="region" aria-label="开工检查清单">
      <div className="panel-head">
        <h3 className="panel-title">开工检查清单</h3>
        <span className="muted">{readiness.ready ? "可以接收工件；等待项完成后自动就绪" : "阻塞项解决后才能接收工件"}</span>
      </div>
      {visibleItems.length > 0 && <ReadinessItems items={visibleItems} />}
    </Card>
  );
  return (
    <section className="readiness readiness-embedded" aria-label="开工检查清单">
      {visibleItems.length > 0 && <ReadinessItems items={visibleItems} compact />}
      {includeReady && readyItems.length > 0 && <details className="readiness-details">
        <summary>已就绪项目（{readyItems.length}）</summary>
        <ReadinessItems items={readyItems} />
      </details>}
    </section>
  );
}

function ReadinessItems({ items, compact = false }: { items: ReadinessItem[]; compact?: boolean }) {
  return <ul className="readiness-list">
    {items.map((item) => (
      <li key={item.key} className={`readiness-item r-${item.state}`}>
        <StatusBadge tone={stateTone[item.state]}>{stateText[item.state]}</StatusBadge>
        <b>{item.title}</b>
        <div className="readiness-detail">
          {item.detail}
          {compact ? (item.hint || (import.meta.env.DEV && item.key === "engine" && item.state !== "ok")) && <ContextHelp label={`${item.title}说明`}>
            {item.hint && <span className="muted readiness-hint">{item.hint}</span>}
            {import.meta.env.DEV && item.key === "engine" && item.state !== "ok" && <span className="muted readiness-hint">{devLyflowHint}</span>}
          </ContextHelp> : <>
            {item.hint && <span className="muted readiness-hint">{item.hint}</span>}
            {import.meta.env.DEV && item.key === "engine" && item.state !== "ok" && <span className="muted readiness-hint">{devLyflowHint}</span>}
          </>}
        </div>
        {item.state !== "ok" && <TargetLink target={item.target} />}
      </li>
    ))}
  </ul>;
}

export function TargetLink({ target }: { target: ReadinessTarget | null }) {
  return target ? <Link to={targetLinks[target].path}>{targetLinks[target].label}</Link> : null;
}
