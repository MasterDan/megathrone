import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { CONNECTION_CHANGED_EVENT } from "@/types";
import type { ConnectionSnapshot, ProxyMode } from "@/types";

/**
 * The long-lived proxy connection. The backend owns the truth (a snapshot is
 * adopted on mount via `connection_status` and every transition arrives as a
 * `connection-changed` event — including unexpected sing-box exits with the
 * stderr tail). This hook only drives the button: busy state, connect,
 * disconnect, and the error surfaced to the user.
 */
export function useConnection() {
  const [snapshot, setSnapshot] = createSignal<ConnectionSnapshot | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const apply = (next: ConnectionSnapshot) => {
    setSnapshot(next);
    if (next.lastError) {
      setError(next.lastError);
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

    // adopt whatever the backend is already doing (page remount, restart)
    void invoke<ConnectionSnapshot | null>("connection_status").then(
      (status) => {
        if (!disposed && status) {
          apply(status);
        }
      },
      (cause) => {
        if (!disposed) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      },
    );

    keep(
      listen<ConnectionSnapshot>(CONNECTION_CHANGED_EVENT, (event) => {
        apply(event.payload);
      }),
    );

    onCleanup(() => {
      disposed = true;
    });
  });

  const connect = async (profileId: number, mode: ProxyMode) => {
    if (busy()) {
      return;
    }
    setError(null);
    setBusy(true);
    try {
      apply(await invoke<ConnectionSnapshot>("connection_connect", { profileId, mode }));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (busy()) {
      return;
    }
    setBusy(true);
    try {
      apply(await invoke<ConnectionSnapshot>("connection_disconnect"));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const dismissError = () => setError(null);

  return { snapshot, busy, error, connect, disconnect, dismissError };
}
