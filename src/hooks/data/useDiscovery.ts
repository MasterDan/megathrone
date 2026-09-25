import { createResource, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { DISCOVERY_PROGRESS_EVENT } from "@/types";
import type { DiscoveryProgress, DiscoveryRunStatus, DiscoverySource } from "@/types";

/** One source's outcome inside the run currently on screen. */
export interface DiscoverySourceProgress {
  status: "pending" | "done" | "failed";
  itemCount: number;
  error: string | null;
}

export interface DiscoveryFinishedSummary {
  created: number;
  updated: number;
  failed: number;
  cancelled: boolean;
}

const normalizeError = (cause: unknown): string =>
  cause instanceof Error ? cause.message : String(cause);

/**
 * The Discovery catalog and its run (one at a time, cancellable): the
 * source list plus CRUD mutations, the live run mirror (per-source
 * progress, counters, testing phase) and adoption of a run still alive
 * in the backend after a page remount. `discovery-progress` events feed
 * the mirror; the final `finished` event refetches the sources (their
 * last-run fields and profile links changed).
 */
export function useDiscovery() {
  const [sources, { refetch }] = createResource(() =>
    invoke<DiscoverySource[]>("discovery_sources_list"),
  );

  const [runStatus, setRunStatus] = createSignal<DiscoveryRunStatus | null>(null);
  const [sourceProgress, setSourceProgress] = createSignal<
    Record<number, DiscoverySourceProgress>
  >({});
  const [lastFinished, setLastFinished] = createSignal<DiscoveryFinishedSummary | null>(null);
  const [cancelling, setCancelling] = createSignal(false);
  const [runError, setRunError] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  const running = () => {
    const status = runStatus();
    return status !== null && status.phase !== "done";
  };

  const applyProgress = (payload: DiscoveryProgress) => {
    switch (payload.kind) {
      case "source-started":
        setSourceProgress((current) => ({
          ...current,
          [payload.sourceId]: { status: "pending", itemCount: 0, error: null },
        }));
        setRunStatus((current) => {
          if (current === null || current.phase === "done") {
            return current;
          }
          return {
            ...current,
            total: Math.max(current.total, current.done + 1),
          };
        });
        break;
      case "source-done":
        setSourceProgress((current) => ({
          ...current,
          [payload.sourceId]: {
            status: payload.ok ? "done" : "failed",
            itemCount: payload.itemCount,
            error: payload.error,
          },
        }));
        setRunStatus((current) => {
          if (current === null || current.phase === "done") {
            return current;
          }
          return {
            ...current,
            done: current.done + 1,
            failed: current.failed + (payload.ok ? 0 : 1),
          };
        });
        break;
      case "scan-started":
        setRunStatus((current) => {
          if (current === null || current.phase === "done") {
            return current;
          }
          return {
            ...current,
            phase: "testing",
            testProfileId: payload.profileId,
            testProfileName: payload.name,
          };
        });
        break;
      case "finished": {
        const cancelled = runStatus()?.cancelled ?? false;
        setLastFinished({
          created: payload.created,
          updated: payload.updated,
          failed: payload.failed,
          cancelled,
        });
        setRunStatus((current) =>
          current === null ? null : { ...current, phase: "done" },
        );
        void refetch();
        break;
      }
    }
  };

  onMount(() => {
    let disposed = false;
    const keep = (promise: Promise<() => void>) => {
      void promise.then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          onCleanup(() => unlisten());
        }
      });
    };

    // adopt a run still alive in the backend (or its finished summary)
    void invoke<DiscoveryRunStatus | null>("discovery_run_status").then(
      (status) => {
        if (disposed || status === null) {
          return;
        }
        setRunStatus(status);
        if (status.phase === "done") {
          setLastFinished({
            created: status.created,
            updated: status.updated,
            failed: status.failed,
            cancelled: status.cancelled,
          });
        }
      },
      (cause) => setRunError(normalizeError(cause)),
    );

    keep(
      listen<DiscoveryProgress>(DISCOVERY_PROGRESS_EVENT, (event) => {
        applyProgress(event.payload);
        if (event.payload.kind === "finished") {
          setCancelling(false);
        }
      }),
    );

    onCleanup(() => {
      disposed = true;
    });
  });

  const run = async (testAfter: boolean) => {
    if (running()) {
      return;
    }
    setRunError(null);
    setLastFinished(null);
    setSourceProgress({});
    setCancelling(false);
    const enabled = (sources() ?? []).filter((source) => source.enabled).length;
    setRunStatus({
      startedAt: new Date().toISOString(),
      total: enabled,
      done: 0,
      failed: 0,
      created: 0,
      updated: 0,
      phase: "fetch",
      testProfileId: null,
      testProfileName: null,
      cancelled: false,
    });
    try {
      await invoke("discovery_run", { testAfter });
    } catch (cause) {
      setRunError(normalizeError(cause));
      setSourceProgress({});
      // a rejected run (e.g. "already running") re-adopts the backend truth
      void invoke<DiscoveryRunStatus | null>("discovery_run_status").then((status) =>
        setRunStatus(status),
      );
    }
  };

  /** Flags the run for a cooperative stop; the final `finished` event
   *  releases the UI (the backend may still finish the source in flight). */
  const cancel = async () => {
    if (!running() || cancelling()) {
      return;
    }
    setCancelling(true);
    setRunStatus((current) =>
      current === null ? current : { ...current, cancelled: true },
    );
    try {
      await invoke("discovery_run_cancel");
    } catch (cause) {
      setRunError(normalizeError(cause));
      setCancelling(false);
      setRunStatus((current) =>
        current === null ? current : { ...current, cancelled: false },
      );
    }
  };

  const mutate = async (action: () => Promise<unknown>) => {
    setError(null);
    setBusy(true);
    try {
      await action();
      await refetch();
      return true;
    } catch (cause) {
      setError(normalizeError(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const addSource = (url: string, name?: string) =>
    mutate(() => invoke<DiscoverySource>("discovery_source_add", { url, name }));

  const updateSource = (
    id: number,
    changes: { url?: string; name?: string; enabled?: boolean },
  ) => mutate(() => invoke<DiscoverySource>("discovery_source_update", { id, ...changes }));

  const deleteSource = (id: number, deleteProfile: boolean) =>
    mutate(() => invoke("discovery_source_delete", { id, deleteProfile }));

  return {
    sources,
    loading: () => sources.loading,
    error,
    busy,
    runStatus,
    running,
    runError,
    dismissRunError: () => setRunError(null),
    cancelling,
    sourceProgress,
    lastFinished,
    refetch,
    run,
    cancel,
    addSource,
    updateSource,
    deleteSource,
  };
}
