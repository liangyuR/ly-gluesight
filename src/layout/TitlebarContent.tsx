import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";

export const TitlebarSlotsContext = createContext<{ details: HTMLElement | null; actions: HTMLElement | null } | null>(null);

export function TitlebarContent({ slot, children }: { slot: "details" | "actions"; children: ReactNode }) {
  const slots = useContext(TitlebarSlotsContext);
  if (!slots) return <>{children}</>;
  return slots[slot] ? createPortal(children, slots[slot]) : null;
}
