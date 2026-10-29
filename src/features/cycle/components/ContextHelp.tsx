import type { ReactNode } from "react";
import { CircleHelp } from "lucide-react";
import { IconButton, Tooltip } from "qing-steel";

export default function ContextHelp({ label, children }: { label: string; children: ReactNode }) {
  return <Tooltip content={children} pinnable delay={0} side="bottom">
    <IconButton className="context-help-trigger" aria-label={label} size="xs" variant="text"><CircleHelp size={15} aria-hidden="true" /></IconButton>
  </Tooltip>;
}
