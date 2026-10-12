import { Link } from "react-router-dom";
import { devLyflowHint, targetLinks } from "../readiness";
import type { Readiness, ReadinessState, ReadinessTarget } from "../types";

const stateText: Record<ReadinessState, string> = { ok: "就绪", waiting: "等待", blocked: "阻塞" };

export default function ReadinessChecklist({ readiness }: { readiness: Readiness }) {
  return (
    <section className="panel readiness" aria-label="开工检查清单">
      <div className="panel-head">
        <h3 className="panel-title">开工检查清单</h3>
        <span className="muted">以下各项全部就绪后才能接收工件</span>
      </div>
      <ul className="readiness-list">
        {readiness.items.map((item) => (
          <li key={item.key} className={`readiness-item r-${item.state}`}>
            <span className="readiness-state">{stateText[item.state]}</span>
            <b>{item.title}</b>
            <span className="readiness-detail">
              {item.detail}
              {item.hint && <span className="muted readiness-hint">{item.hint}</span>}
              {import.meta.env.DEV && item.key === "engine" && item.state !== "ok" && <span className="muted readiness-hint">{devLyflowHint}</span>}
            </span>
            {item.state !== "ok" && <TargetLink target={item.target} />}
          </li>
        ))}
      </ul>
    </section>
  );
}

export function TargetLink({ target }: { target: ReadinessTarget | null }) {
  return target ? <Link to={targetLinks[target].path}>{targetLinks[target].label}</Link> : null;
}
