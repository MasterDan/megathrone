import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

import type { DpiStrategy, DpiSettings, DpiTestResult } from "@/types";

/**
 * byedpi strategies (Settings → DPI): the list, CRUD mutations and the active
 * selection. Mutations return true on success; the list is refreshed from
 * the backend afterwards (it owns the truth). `applyTestResult` merges a
 * live strategy-test outcome into the list without a refetch.
 */
export function useDpiStrategies() {
  const [strategies, setStrategies] = createSignal<DpiStrategy[]>([]);
  const [loading, setLoading] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    try {
      setStrategies(await invoke<DpiStrategy[]>("dpi_list_strategies"));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  const run = async (action: () => Promise<unknown>) => {
    setError(null);
    setBusy(true);
    try {
      await action();
      await refresh();
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const add = (name: string, args: string) =>
    run(() => invoke("dpi_add_strategy", { name, args }));

  const update = (id: number, name: string, args: string) =>
    run(() => invoke("dpi_update_strategy", { id, name, args }));

  const remove = (id: number) => run(() => invoke("dpi_delete_strategy", { id }));

  const select = (id: number) => run(() => invoke("dpi_select_strategy", { id }));

  const applyTestResult = (result: DpiTestResult) => {
    setStrategies((previous) =>
      previous.map((strategy) =>
        strategy.id === result.strategyId
          ? {
              ...strategy,
              urlOk: result.urlOk,
              urlTotal: result.urlTotal,
              tested: result.error ? strategy.tested : Math.max(1, strategy.tested),
            }
          : strategy,
      ),
    );
  };

  onMount(() => {
    void refresh();
    onCleanup(() => {});
  });

  return {
    strategies,
    loading,
    busy,
    error,
    add,
    update,
    remove,
    select,
    applyTestResult,
    refresh,
  };
}

/**
 * DPI tunnel settings (Settings → DPI): the ciadpi listen port and the DPI
 * master toggle.
 */
export function useDpiSettings() {
  const [settings, setSettings] = createSignal<DpiSettings | null>(null);
  const [loading, setLoading] = createSignal(true);
  // busy state is per control: the port input must not flash its disabled
  // style while the DPI toggle round-trips (and vice versa)
  const [portBusy, setPortBusy] = createSignal(false);
  const [toggleBusy, setToggleBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    try {
      setSettings(await invoke<DpiSettings>("dpi_get_settings"));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  const setPort = async (port: number) => {
    setError(null);
    setPortBusy(true);
    try {
      setSettings(await invoke<DpiSettings>("dpi_set_port", { port }));
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setPortBusy(false);
    }
  };

  /** Master toggle: off — connecting is proxy-only, no byedpi tunnel. */
  const setEnabled = async (enabled: boolean) => {
    setError(null);
    setToggleBusy(true);
    try {
      setSettings(await invoke<DpiSettings>("dpi_set_enabled", { enabled }));
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setToggleBusy(false);
    }
  };

  onMount(() => {
    void refresh();
    onCleanup(() => {});
  });

  return { settings, loading, portBusy, toggleBusy, error, setPort, setEnabled, refresh };
}
