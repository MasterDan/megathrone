import { createEffect, on, untrack } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { useDragAndDrop } from "@formkit/drag-and-drop/solid";

import type { ProfileSummary } from "@/types";

/**
 * Drag-and-drop profile ordering over a filtered view of the profile list
 * (the sidebar drags only its visible profiles, the settings page drags
 * all of them). The id store is the drag source of truth; a finished drag
 * persists the new order (`profile_set_sidebar_order`), the backend emits
 * `profiles-changed`, the caller's resource refetches and the sync effect
 * sees the persisted order and leaves the store alone. A failed persist
 * refetches too, so the store falls back to the DB truth.
 *
 * Drags start only from a `.drag-handle` element — rows keep their
 * buttons and toggles clickable.
 */
export function useProfileOrder(
  profiles: () => ProfileSummary[] | undefined,
  refetch: () => void,
  include: (profile: ProfileSummary) => boolean,
) {
  const [parent, ids, setIds] = useDragAndDrop<HTMLElement, number>([], {
    dragHandle: ".drag-handle",
    draggingClass: "opacity-40",
    onSort: (data) => {
      void invoke("profile_set_sidebar_order", { orderedIds: data.values }).catch(() =>
        refetch(),
      );
    },
  });

  // adopts the persisted order (and any list change): after a drag the
  // refetched order equals the store, so nothing flickers. The store is
  // read untracked — a drag's own store write must not pull stale
  // resource data back in.
  createEffect(
    on(
      () => {
        const list = profiles();
        return list ? list.filter(include).map((profile) => profile.id) : undefined;
      },
      (fresh) => {
        if (!fresh) {
          return;
        }
        const current = untrack(ids);
        if (fresh.length === current.length && fresh.every((id, i) => id === current[i])) {
          return;
        }
        setIds(fresh);
      },
      { defer: true },
    ),
  );

  return { parent, ids };
}
