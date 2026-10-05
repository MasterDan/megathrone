import { createResource, onCleanup, onMount } from "solid-js";

import { usePlatform } from "@/platform/context";
import { PROFILES_CHANGED_EVENT } from "@/types";
import type { ProfileSummary } from "@/types";

export function useProfiles() {
  const platform = usePlatform();
  const [profiles, { refetch }] = createResource(() =>
    platform.invoke<ProfileSummary[]>("profiles_list"),
  );

  // any mutation (own or a background auto-update) refreshes the list
  onMount(() => {
    let disposed = false;
    void platform.listen(PROFILES_CHANGED_EVENT, () => void refetch()).then((unlisten) => {
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
