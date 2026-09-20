import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { PROFILES_CHANGED_EVENT } from "@/types";
import type { ProfilesChangedPayload } from "@/types";

/**
 * True while this profile is being updated right now — through the Update
 * button or by the background scheduler. The backend announces both via
 * `profiles-changed` (kinds "updating" / "update-done"), so the spinner is
 * honest about automatic refreshes too. A page mounted in the middle of an
 * update adopts the state via `profile_update_status`.
 */
export function useProfileUpdating(profileId: () => number) {
  const [updating, setUpdating] = createSignal(false);

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

    // adopt an update that is already running in the backend
    void invoke<boolean>("profile_update_status", { profileId: profileId() }).then((active) => {
      if (!disposed) {
        setUpdating(active);
      }
    });

    keep(
      listen<ProfilesChangedPayload>(PROFILES_CHANGED_EVENT, (event) => {
        const payload = event.payload;
        if (payload.profileId !== profileId()) {
          return;
        }
        if (payload.kind === "updating") {
          setUpdating(true);
        } else if (payload.kind === "update-done") {
          setUpdating(false);
        }
      }),
    );

    onCleanup(() => {
      disposed = true;
    });
  });

  return { updating };
}
