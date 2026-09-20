import { createResource, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { LATENCY_PROGRESS_EVENT, PROFILES_CHANGED_EVENT } from "@/types";
import type {
  EndpointItem,
  LatencyProgress,
  ProfilesChangedPayload,
  SelectionMode,
} from "@/types";

const isSelectionMode = (value: string): value is SelectionMode =>
  value === "round_robin" ||
  value === "fastest" ||
  value === "most_available" ||
  value === "manual";

/**
 * The profile's endpoint selection: which endpoint runs (persisted in the
 * DB by raw link, so it survives restarts and content updates) and how it
 * is chosen (the strategy tabs — automatic strategies keep it fed from the
 * background, so the value can change on its own). A null profile id (no
 * profile chosen yet) leaves both resources unresolved.
 */
export function useProfileSelection(profileId: () => number | null) {
  const [selected, { refetch: refetchSelected, mutate }] = createResource(
    profileId,
    (id) =>
      id === null
        ? Promise.resolve<EndpointItem | null>(null)
        : invoke<EndpointItem | null>("profile_selected_endpoint", { profileId: id }),
  );
  const [mode, { refetch: refetchMode }] = createResource(profileId, (id) =>
    id === null
      ? Promise.resolve<SelectionMode | null>(null)
      : invoke<string>("profile_selection_mode", { profileId: id }).then((value) =>
          isSelectionMode(value) ? value : "fastest",
        ),
  );

  const refresh = () => {
    void refetchSelected();
    void refetchMode();
  };

  /** Picks a specific endpoint — an explicit choice always switches the
   *  profile to manual (like tapping a server in Hiddify). */
  const select = async (item: EndpointItem) => {
    mutate(item);
    try {
      await invoke("profile_select_endpoint", { profileId: profileId(), itemId: item.id });
    } finally {
      refresh();
    }
  };

  const clear = async () => {
    mutate(null);
    try {
      await invoke("profile_select_endpoint", { profileId: profileId(), itemId: null });
    } finally {
      refresh();
    }
  };

  /** Switches the strategy (the tabs). */
  const setMode = async (next: SelectionMode) => {
    try {
      await invoke("profile_set_selection_mode", { profileId: profileId(), mode: next });
    } finally {
      refresh();
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

    keep(
      listen<ProfilesChangedPayload>(PROFILES_CHANGED_EVENT, (event) => {
        const payload = event.payload;
        if (payload.profileId !== profileId()) {
          return;
        }
        if (payload.kind === "content" || payload.kind === "selection") {
          // re-resolve the raw-link key (and the mode) against possibly new
          // endpoints — automatic strategies flip both on their own
          refresh();
        }
      }),
    );

    keep(
      listen<LatencyProgress>(LATENCY_PROGRESS_EVENT, (event) => {
        const payload = event.payload;
        const current = selected.latest;
        if (!current || payload.profileId !== profileId()) {
          return;
        }
        const hit = payload.results.find((result) => result.id === current.id);
        if (hit) {
          mutate({
            ...current,
            available: hit.available,
            latencyMs: hit.latencyMs,
            urlOk: hit.urlOk,
            urlTotal: hit.urlTotal,
          });
        }
      }),
    );

    onCleanup(() => {
      disposed = true;
    });
  });

  return { selected, mode, select, clear, setMode };
}
