import { useCallback, useEffect, useMemo, useState } from "react";
import { errorText } from "../../lib/errors";
import { subscribe } from "../plc";
import { historyApi } from "./api";
import type { PartDetail } from "./types";

type OverviewActions = {
  retry: () => void;
  setMutationBusy: (busy: boolean) => void;
  saveTags: (tags: string[]) => void;
  stopRefreshing: () => void;
};

type OverviewScope = { actions: OverviewActions | null };
type OverviewState = { scope: OverviewScope; detail: PartDetail | null; error: string };

export function useHistoryOverview(recordId: number) {
  const scope = useMemo<OverviewScope>(() => ({ actions: null }), [recordId]);
  const [state, setState] = useState<OverviewState | null>(null);

  useEffect(() => {
    let stopped = false;
    let listenersLeft = 2;
    let mutationBusy = false;
    let active: object | null = null;
    let preserveDetailOnNextError: boolean | null = null;
    let cycleId: string | null | undefined;
    const updatesBeforeIdentity = new Set<string>();

    const load = () => {
      if (stopped || mutationBusy || active || preserveDetailOnNextError === null) return;
      const preserveDetail = preserveDetailOnNextError;
      preserveDetailOnNextError = null;
      const request = {};
      active = request;
      void historyApi.overview(recordId).then(next => {
        if (active !== request) return;
        cycleId = next.summary.cycleId;
        if (cycleId && updatesBeforeIdentity.has(cycleId)) refresh();
        updatesBeforeIdentity.clear();
        setState({ scope, detail: next, error: "" });
      }).catch(cause => {
        if (active !== request) return;
        updatesBeforeIdentity.clear();
        setState(previous => ({
          scope,
          detail: preserveDetail && previous?.scope === scope ? previous.detail : null,
          error: errorText(cause),
        }));
      }).finally(() => {
        if (active !== request) return;
        active = null;
        load();
      });
    };

    const refresh = (preserveDetail = false) => {
      if (stopped || listenersLeft > 0) return;
      preserveDetailOnNextError = (preserveDetailOnNextError ?? true) && preserveDetail;
      load();
    };

    const stopRefreshing = () => {
      stopped = true;
      active = null;
      preserveDetailOnNextError = null;
      updatesBeforeIdentity.clear();
    };

    scope.actions = {
      retry: () => {
        setState(previous => previous?.scope === scope ? { ...previous, error: "" } : previous);
        refresh();
      },
      setMutationBusy: busy => {
        if (stopped || mutationBusy === busy) return;
        mutationBusy = busy;
        if (busy && active) {
          active = null;
          preserveDetailOnNextError ??= true;
        }
        if (!busy) load();
      },
      saveTags: tags => {
        if (stopped) return;
        setState(previous => previous?.scope === scope && previous.detail
          ? { ...previous, detail: { ...previous.detail, summary: { ...previous.detail.summary, tags } } }
          : previous);
        refresh();
      },
      stopRefreshing,
    };

    const listenerSettled = () => { if (--listenersLeft === 0) refresh(); };
    const stopUpdated = subscribe<string>("history://updated", updatedCycleId => {
      if (cycleId === updatedCycleId) refresh(mutationBusy);
      else if (cycleId === undefined && active) updatesBeforeIdentity.add(updatedCycleId);
    }, undefined, listenerSettled);
    const stopChanged = subscribe<number[]>("history://changed", ids => {
      if (ids.includes(recordId)) refresh(mutationBusy);
    }, undefined, listenerSettled);

    return () => {
      stopRefreshing();
      scope.actions = null;
      stopUpdated();
      stopChanged();
    };
  }, [recordId, scope]);

  const displayed = state?.scope === scope ? state : null;
  const retry = useCallback(() => scope.actions?.retry(), [scope]);
  const setMutationBusy = useCallback((busy: boolean) => scope.actions?.setMutationBusy(busy), [scope]);
  const saveTags = useCallback((tags: string[]) => scope.actions?.saveTags(tags), [scope]);
  const stopRefreshing = useCallback(() => scope.actions?.stopRefreshing(), [scope]);
  return { detail: displayed?.detail ?? null, error: displayed?.error ?? "", retry, setMutationBusy, saveTags, stopRefreshing };
}
