import { createContext, useContext, useLayoutEffect, useRef, useState, type PropsWithChildren } from "react";
import { useNavigate } from "react-router-dom";
import { Button, Card, IconButton } from "qing-steel";
import { X } from "lucide-react";
import { historyApi } from "../../history/api";
import { historyReturnSearch } from "../../history/returnContext";
import type { HistoryDetailState } from "../../history/types";
import { errorText } from "../../../lib/errors";

interface EvidenceLocation {
  cycleId: string;
  sn: string;
  seq: number;
  camera?: number;
  cameraName?: string;
}

const PreviousEvidenceContext = createContext<{
  lastEvidence: EvidenceLocation | null;
  setLastEvidence: (location: EvidenceLocation | null) => void;
} | null>(null);

export function PreviousEvidenceProvider({ children }: PropsWithChildren) {
  const [lastEvidence, setLastEvidence] = useState<EvidenceLocation | null>(null);
  return <PreviousEvidenceContext.Provider value={{ lastEvidence, setLastEvidence }}>{children}</PreviousEvidenceContext.Provider>;
}

export function usePreviousEvidence() {
  const state = useContext(PreviousEvidenceContext);
  if (!state) throw new Error("在线证据状态未初始化");
  return state;
}

export default function PreviousEvidence({ evidence, onDismiss }: { evidence: EvidenceLocation; onDismiss: () => void }) {
  const navigate = useNavigate();
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState("");
  const inFlight = useRef(false);
  const alive = useRef(false);
  useLayoutEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  async function open() {
    if (inFlight.current) return;
    inFlight.current = true;
    setOpening(true);
    setError("");
    try {
      const { cycleId, camera, seq } = evidence;
      const id = await historyApi.idByCycle(cycleId);
      if (!alive.current) return;
      if (id === null) {
        setError("这件的历史记录暂不可用，可稍后重试。");
        return;
      }
      navigate({ pathname: `/history/${id}`, search: historyReturnSearch({ camera: camera ?? 0, seq, candidate: null, comparison: null, original: true }) }, {
        state: { returnTo: { path: "/inspect", label: "返回在线检测" } } satisfies HistoryDetailState,
      });
    } catch (cause) {
      if (alive.current) setError(errorText(cause));
    } finally {
      inFlight.current = false;
      if (alive.current) setOpening(false);
    }
  }

  const caption = `SN ${evidence.sn || "—"} · 帧组 ${evidence.seq}${evidence.cameraName ? ` · ${evidence.cameraName}` : ""}`;
  return <Card role="region" aria-label="上次定位的检测证据">
    <div className="panel-head">
      <Button size="sm" variant="tonal" loading={opening} disabled={opening} onClick={open}>继续查看证据</Button>
      <span className="spacer" />
      <IconButton size="sm" variant="text" aria-label="清除回查位置" onClick={onDismiss}><X size={14} aria-hidden="true" /></IconButton>
    </div>
    <div className="muted mono prod-evidence-location">{caption}</div>
    {error && <div className="muted" role="status">{error}</div>}
  </Card>;
}
