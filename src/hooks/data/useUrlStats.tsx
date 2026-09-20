import type { Accessor, ParentComponent } from "solid-js";
import { createContext, createSignal, onCleanup, onMount, useContext } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { CONNECTION_CHANGED_EVENT, TRAFFIC_STATS_EVENT } from "@/types";
import type { UrlStatEntry } from "@/types";

interface UrlStatsStore {
  entries: Accessor<readonly UrlStatEntry[]>;
}

const UrlStatsContext = createContext<UrlStatsStore>();

/**
 * Live per-URL session summary: which host was reached through which tunnel
 * (proxy / DPI / direct) and how many requests went there. The backend folds
 * the numbers off the same clash API snapshots that feed the traffic charts;
 * this provider adopts the table on mount and refetches on every traffic
 * tick, so it stays current while the session runs — and freezes at the last
 * snapshot after a disconnect (a fresh connect resets it, matching the
 * backend). Mounted once in `AppLayout`, above the routes, so the page keeps
 * its data across navigation — the same pattern as `TrafficStatsProvider`.
 */
export const UrlStatsProvider: ParentComponent = (props) => {
  const [entries, setEntries] = createSignal<readonly UrlStatEntry[]>([]);

  onMount(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];

    const refresh = () => {
      void invoke<UrlStatEntry[]>("connection_url_stats").then(
        (next) => {
          if (!disposed) {
            setEntries(next);
          }
        },
        () => {
          // best-effort: the charts keep running, the summary catches up
        },
      );
    };

    refresh();
    // every traffic tick may have folded new connections in; a session
    // change (fresh connect resets, disconnect freezes) refetches too
    for (const event of [TRAFFIC_STATS_EVENT, CONNECTION_CHANGED_EVENT]) {
      void listen(event, refresh).then((stop) => {
        if (disposed) {
          stop();
        } else {
          unlisteners.push(stop);
        }
      });
    }

    onCleanup(() => {
      disposed = true;
      for (const stop of unlisteners) {
        stop();
      }
    });
  });

  return <UrlStatsContext.Provider value={{ entries }}>{props.children}</UrlStatsContext.Provider>;
};

export function useUrlStats(): UrlStatsStore {
  const store = useContext(UrlStatsContext);
  if (!store) {
    throw new Error("useUrlStats must be used within UrlStatsProvider");
  }
  return store;
}
