import type { Accessor } from "solid-js";
import { createResource, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { PROFILES_CHANGED_EVENT } from "@/types";
import type { ProfileSummary } from "@/types";

export function useProfile(profileId: Accessor<number>) {
  const [profile, { refetch }] = createResource(profileId, (id) =>
    invoke<ProfileSummary>("profile_get", { profileId: id }),
  );

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
    profile,
    loading: () => profile.loading,
    error: () => profile.error,
  };
}
