import { createResource, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { PROFILES_CHANGED_EVENT } from "@/types";
import type { ProfileSummary } from "@/types";

export function useProfiles() {
  const [profiles, { refetch }] = createResource(() => invoke<ProfileSummary[]>("profiles_list"));

  // any mutation (own or a background auto-update) refreshes the list
  onMount(() => {
    let disposed = false;
    void listen(PROFILES_CHANGED_EVENT, () => void refetch()).then((unlisten) => {
      if (disposed) {
        unlisten();
      }
      onCleanup(() => unlisten());
    });
    onCleanup(() => {
      disposed = true;
    });
  });

  return {
    profiles,
    loading: () => profiles.loading,
    error: () => profiles.error,
    refetch,
  };
}
