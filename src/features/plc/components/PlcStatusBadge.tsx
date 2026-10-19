import { StatusBadge } from "qing-steel";
import { linkStateLabels } from "../meta";
import type { LinkState } from "../types";

const tones = { connected: "ok", connecting: "warn", error: "ng", disconnected: "off" } as const;

export default function PlcStatusBadge({ state }: { state: LinkState | undefined }) {
  const s = state ?? "disconnected";
  return <StatusBadge tone={tones[s]}>{linkStateLabels[s]}</StatusBadge>;
}
