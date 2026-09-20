import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { TEST_SITES_CHANGED_EVENT } from "@/types";
import type { RouteAction, RoutingConfig } from "@/types";

/**
 * Routing (Settings → Routing): every URL category's outbound (proxy, DPI
 * tunnel or direct) plus the fallback action for everything else. The
 * per-category switches are flipped through `useTestSites.setAction` —
 * the config is re-read here after every mutation via the backend's
 * `test-sites-changed` event.
 */
export function useRouting() {
  const [config, setConfig] = createSignal<RoutingConfig | null>(null);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    try {
      setConfig(await invoke<RoutingConfig>("routing_list"));
      setError(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  const setFallback = async (action: RouteAction) => {
    setError(null);
    try {
      await invoke("routing_set_fallback", { action });
      await refresh();
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    }
  };

  onMount(() => {
    void refresh();
    // category edits (add/delete/rename, action flips from other views)
    // reshape this config
    let disposed = false;
    void listen(TEST_SITES_CHANGED_EVENT, () => void refresh()).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        onCleanup(() => unlisten());
      }
    });
    onCleanup(() => {
      disposed = true;
    });
  });

  return { config, loading, error, setFallback, refresh };
}
