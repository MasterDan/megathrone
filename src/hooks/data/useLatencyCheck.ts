import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { LATENCY_PROGRESS_EVENT, PROFILES_CHANGED_EVENT } from "@/types";
import type { LatencyProgress, LatencyStatus, LatencySummary } from "@/types";
import type { ProfilesChangedPayload } from "@/types";

/**
 * Whole-profile availability scan. The backend persists per-endpoint
 * availability/latency and emits `profiles-changed` (kind "latency") when
 * done — endpoint lists pick that up on their own. This hook only drives the
 * button: busy state, live progress and cancellation. A scan started earlier
 * (the user left the page and came back) is adopted on mount via
 * `profile_latency_status`, so the indicator never lies.
 */
export function useLatencyCheck(profileId: () => number) {
  const [checking, setChecking] = createSignal(false);
  const [stopping, setStopping] = createSignal(false);
  const [progress, setProgress] = createSignal<LatencyProgress | null>(null);
  const [error, setError] = createSignal<string | null>(null);

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

    // adopt a scan that is already running in the backend
    void invoke<LatencyStatus | null>("profile_latency_status", { profileId: profileId() }).then(
      (status) => {
        if (disposed || status === null) {
          return;
        }
        setChecking(true);
        setProgress({ profileId: profileId(), done: status.done, total: status.total, results: [] });
      },
      (cause) => setError(cause instanceof Error ? cause.message : String(cause)),
    );

    keep(
      listen<LatencyProgress>(LATENCY_PROGRESS_EVENT, (event) => {
        if (event.payload.profileId === profileId()) {
          setChecking(true);
          setProgress(event.payload);
        }
      }),
    );

    // the scan is over (finished, cancelled or failed) — release the button
    keep(
      listen<ProfilesChangedPayload>(PROFILES_CHANGED_EVENT, (event) => {
        const payload = event.payload;
        if (payload.profileId === profileId() && payload.kind === "latency") {
          setChecking(false);
          setStopping(false);
          setProgress(null);
        }
      }),
    );

    onCleanup(() => {
      disposed = true;
    });
  });

  const check = async () => {
    if (checking()) {
      return;
    }
    setError(null);
    setChecking(true);
    setStopping(false);
    setProgress(null);
    try {
      await invoke<LatencySummary>("profile_check_latency", { profileId: profileId() });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setChecking(false);
      setStopping(false);
      setProgress(null);
    }
  };

  /** Stops the running scan; the button is released by the finishing command
   *  (own `check`) or by the final `profiles-changed` event (adopted scans). */
  const cancel = async () => {
    if (!checking() || stopping()) {
      return;
    }
    setStopping(true);
    try {
      await invoke("profile_cancel_latency", { profileId: profileId() });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setStopping(false);
    }
  };

  return { checking, stopping, progress, error, check, cancel };
}
