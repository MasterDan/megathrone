import { createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

import type { GeneralSettings } from "@/types";

/**
 * General settings (Settings → General): the automatic-strategy cadences
 * (endpoint rotation, reachable re-checks), the close-to-tray behavior
 * and the raw local proxy.
 */
export function useGeneralSettings() {
  const [settings, setSettings] = createSignal<GeneralSettings | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    try {
      setSettings(await invoke<GeneralSettings>("settings_get_general"));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  /** Saves the Automatic-strategies card (cadences + availability floor). */
  const saveCadences = async (
    rotationMinutes: number,
    recheckMinutes: number,
    minAvailabilityPercent: number,
  ) => {
    setError(null);
    setBusy(true);
    try {
      setSettings(
        await invoke<GeneralSettings>("settings_set_cadences", {
          rotationMinutes,
          recheckMinutes,
          minAvailabilityPercent,
        }),
      );
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  /** Saves the Close behavior card (the toggle commits immediately). */
  const saveCloseToTray = async (closeToTray: boolean) => {
    setError(null);
    setBusy(true);
    try {
      setSettings(
        await invoke<GeneralSettings>("settings_set_close_to_tray", { closeToTray }),
      );
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  /** Saves the Local proxy card (toggle + port); while connected, a change
   *  that alters the running config reconnects the session (the outcome
   *  arrives as a restart-result toast). */
  const saveRawProxy = async (rawProxyEnabled: boolean, rawProxyPort: number) => {
    setError(null);
    setBusy(true);
    try {
      setSettings(
        await invoke<GeneralSettings>("settings_set_raw_proxy", {
          rawProxyEnabled,
          rawProxyPort,
        }),
      );
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  onMount(() => {
    void refresh();
  });

  return {
    settings,
    loading,
    busy,
    error,
    saveCadences,
    saveCloseToTray,
    saveRawProxy,
    refresh,
  };
}
