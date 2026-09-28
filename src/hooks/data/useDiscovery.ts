import { createResource, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { DISCOVERY_PROGRESS_EVENT } from "@/types";
import type {
  DiscoveryProgress,
  DiscoveryRunStatus,
  DiscoverySettings,
  DiscoverySource,
} from "@/types";

/** One source's outcome inside the run currently on screen. */
export interface DiscoverySourceProgress {
  status: "pending" | "done" | "failed";
  itemCount: number;
  error: string | null;
}

/** One merge group's outcome — its shared profile was stored (ok) or the
 *  unit failed (failed store / every member failed). */
export interface DiscoveryGroupProgress {
  ok: boolean;
  itemCount: number;
  error: string | null;
}

/** Collapses a source list into run units: an ungrouped source is one,
 *  a merge group (all its members) is one shared profile. */
export const countRunUnits = (list: DiscoverySource[]): number => {
  const groups = new Set<string>();
  let units = 0;
  for (const source of list) {
    if (source.mergeGroup !== null) {
      groups.add(source.mergeGroup);
    } else {
      units += 1;
    }
  }
  return units + groups.size;
};

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
  const [groupProgress, setGroupProgress] = createSignal<
    Record<string, DiscoveryGroupProgress>
  >({});
  const [lastFinished, setLastFinished] = createSignal<DiscoveryFinishedSummary | null>(null);
  const [cancelling, setCancelling] = createSignal(false);
  const [runError, setRunError] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [testAfter, setTestAfter] = createSignal(false);
  const [autoUpdateMinutes, setAutoUpdateMinutes] = createSignal<number | null>(null);
  const [savingSettings, setSavingSettings] = createSignal(false);

  const running = () => {
    const status = runStatus();
    return status !== null && status.phase !== "done";
  };

  /** Whether a source id is its own run unit (not a merge-group member). */
  const isUngrouped = (sourceId: number) => {
    const source = (sources() ?? []).find((entry) => entry.id === sourceId);
    return source !== undefined && source.mergeGroup === null;
  };

  const applyProgress = (payload: DiscoveryProgress) => {
    switch (payload.kind) {
      case "source-started":
        setSourceProgress((current) => ({
          ...current,
          [payload.sourceId]: { status: "pending", itemCount: 0, error: null },
        }));
        // only an ungrouped source is its own run unit; a group's members
        // never grow the total
        if (isUngrouped(payload.sourceId)) {
          setRunStatus((current) => {
            if (current === null || current.phase === "done") {
              return current;
            }
            return {
              ...current,
              total: Math.max(current.total, current.done + 1),
            };
          });
        }
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
        // `done` follows run units: an ungrouped source completes itself,
        // a group's unit completes on its group-done event (a failed
        // member URL still counts as a failure right away)
        setRunStatus((current) => {
          if (current === null || current.phase === "done") {
            return current;
          }
          return {
            ...current,
            done: current.done + (isUngrouped(payload.sourceId) ? 1 : 0),
            failed: current.failed + (payload.ok ? 0 : 1),
          };
        });
        break;
      case "group-done":
        setGroupProgress((current) => ({
          ...current,
          [payload.group]: {
            ok: payload.ok,
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

    // the persisted run options (the "test after" default, the interval)
    void invoke<DiscoverySettings>("discovery_get_settings").then(
      (settings) => {
        if (disposed) {
          return;
        }
        setTestAfter(settings.testAfter);
        setAutoUpdateMinutes(settings.autoUpdateMinutes);
      },
      (cause) => setError(normalizeError(cause)),
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
    setGroupProgress({});
    setCancelling(false);
    const enabled = (sources() ?? []).filter((source) => source.enabled);
    setRunStatus({
      startedAt: new Date().toISOString(),
      total: countRunUnits(enabled),
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

  /** Saves the run settings as a merged update — unlike the source
   *  mutations this never refetches the list (the settings don't affect
   *  it); the backend's re-loaded state lands in the signals. */
  const saveSettings = async (patch: Partial<DiscoverySettings>): Promise<boolean> => {
    setError(null);
    setSavingSettings(true);
    try {
      const settings = await invoke<DiscoverySettings>("discovery_set_settings", {
        settings: {
          testAfter: testAfter(),
          autoUpdateMinutes: autoUpdateMinutes(),
          ...patch,
        },
      });
      setTestAfter(settings.testAfter);
      setAutoUpdateMinutes(settings.autoUpdateMinutes);
      return true;
    } catch (cause) {
      setError(normalizeError(cause));
      return false;
    } finally {
      setSavingSettings(false);
    }
  };

  const saveTestAfter = (value: boolean) => saveSettings({ testAfter: value });
  const saveAutoUpdate = (minutes: number | null) =>
    saveSettings({ autoUpdateMinutes: minutes });

  const addSource = (url: string, name?: string, mergeGroup?: string) =>
    mutate(() =>
      invoke<DiscoverySource>("discovery_source_add", { url, name, mergeGroup }),
    );

  const updateSource = (
    id: number,
    changes: { url?: string; name?: string; enabled?: boolean },
  ) => mutate(() => invoke<DiscoverySource>("discovery_source_update", { id, ...changes }));

  const deleteSource = (id: number, deleteProfile: boolean) =>
    mutate(() => invoke("discovery_source_delete", { id, deleteProfile }));

  /** Removes a whole merge group with its shared profile. */
  const deleteGroup = (group: string) =>
    mutate(() => invoke("discovery_source_delete_group", { group }));

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
    groupProgress,
    lastFinished,
    refetch,
    run,
    cancel,
    testAfter,
    autoUpdateMinutes,
    savingSettings,
    saveTestAfter,
    saveAutoUpdate,
    addSource,
    updateSource,
    deleteSource,
    deleteGroup,
  };
}
