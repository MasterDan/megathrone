import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { TbOutlineGripVertical, TbOutlineLayoutList, TbOutlineListCheck, TbOutlineTrash } from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { useProfileActions } from "@/hooks/data/useProfileActions";
import { useProfiles } from "@/hooks/data/useProfiles";
import { useProfileOrder } from "@/hooks/useProfileOrder";
import type { ProfileSummary } from "@/types";

/**
 * Settings → Profiles: the sidebar's profile list management — drag rows
 * into the order the sidebar shows (the drag persists immediately), flip
 * a profile's sidebar visibility, and bulk-delete via checkboxes (the
 * delete button appears once something is checked). The list refresh
 * arrives via the profiles-changed event, so the actions hook gets a
 * no-op onChanged (mirrors EditProfileModal).
 */
export const ProfilesSection: Component = () => {
  const { profiles, loading, refetch } = useProfiles();
  const actions = useProfileActions(() => {});
  const { parent, ids } = useProfileOrder(
    profiles,
    () => void refetch(),
    () => true,
  );

  const byId = createMemo(() => {
    const map = new Map<number, ProfileSummary>();
    for (const profile of profiles() ?? []) {
      map.set(profile.id, profile);
    }
    return map;
  });

  const [checked, setChecked] = createSignal<ReadonlySet<number>>(new Set());
  const [confirming, setConfirming] = createSignal(false);

  const checkedCount = () => checked().size;

  // keep the count honest: a profile deleted elsewhere leaves the set
  createEffect(() => {
    const present = new Set<number>((profiles() ?? []).map((profile) => profile.id));
    setChecked((prev) => {
      const next = new Set<number>([...prev].filter((id) => present.has(id)));
      return next.size === prev.size ? prev : next;
    });
  });

  const toggleChecked = (id: number) => {
    setChecked((prev) => {
      const next = new Set<number>(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const allChecked = () => {
    const list = ids();
    return list.length > 0 && list.every((id) => checked().has(id));
  };

  const toggleAll = () => {
    setChecked(allChecked() ? new Set<number>() : new Set<number>(ids()));
  };

  const deleteSelected = async () => {
    const picked = [...checked()];
    if (picked.length === 0) {
      return;
    }
    if (await actions.removeBulk(picked)) {
      setConfirming(false);
      setChecked(new Set<number>());
    }
  };

  return (
    <section class="flex flex-col gap-4">
      <div class="card bg-base-content/5 shadow-sm">
        <div class="card-body gap-4 p-5">
          <div>
            <h2 class="flex items-center gap-2 text-lg font-bold">
              <TbOutlineLayoutList size={20} class="text-base-content/50" />
              Profiles
            </h2>
            <p class="mt-1 text-sm text-base-content/60">
              The sidebar's list: drag rows into the order you want (the drag
              saves immediately), hide profiles from the sidebar without
              deleting them, and check a few off to remove them in bulk.
              Deleting the connected profile's session takes it down first.
            </p>
          </div>

          <Show
            when={!loading()}
            fallback={<span class="loading loading-spinner loading-sm self-center" />}
          >
            <Show
              when={confirming()}
              fallback={
                <div class="flex flex-wrap items-center gap-2">
                  <Button
                    ghost
                    size="sm"
                    class="gap-1"
                    disabled={ids().length === 0}
                    onClick={toggleAll}
                  >
                    <TbOutlineListCheck size={16} />
                    {allChecked() ? "Clear" : "Select all"}
                  </Button>
                  <Show when={checkedCount() > 0}>
                    <Button
                      variant="error"
                      size="sm"
                      class="gap-1"
                      disabled={actions.busy()}
                      onClick={() => setConfirming(true)}
                    >
                      <TbOutlineTrash size={16} />
                      Delete {checkedCount()}
                    </Button>
                  </Show>
                </div>
              }
            >
              <div class="flex flex-wrap items-center gap-2">
                <span class="text-sm font-medium text-error">
                  Delete {checkedCount()}{" "}
                  {checkedCount() === 1 ? "profile" : "profiles"}?
                </span>
                <Button
                  variant="error"
                  size="sm"
                  loading={actions.busy()}
                  disabled={checkedCount() === 0}
                  onClick={() => void deleteSelected()}
                >
                  Yes, delete
                </Button>
                <Button ghost size="sm" onClick={() => setConfirming(false)}>
                  No
                </Button>
              </div>
            </Show>

            <Show
              when={ids().length > 0}
              fallback={
                <p class="text-sm text-base-content/50">
                  No profiles yet — add one from the sidebar's Add profile button.
                </p>
              }
            >
              <ul ref={parent} class="flex flex-col gap-1">
                <For each={ids()}>
                  {(id) => (
                    <Show when={byId().get(id)} keyed>
                      {(profile) => (
                        <li
                          class="flex items-center gap-3 rounded-xl bg-base-content/5 px-3 py-2"
                          classList={{ "opacity-60": !profile.sidebarVisible }}
                        >
                          <span
                            class="drag-handle flex size-5 shrink-0 cursor-grab items-center justify-center text-base-content/40 transition-colors hover:text-base-content/80 active:cursor-grabbing"
                            title="Drag to reorder"
                          >
                            <TbOutlineGripVertical size={16} />
                          </span>
                          <input
                            type="checkbox"
                            class="checkbox checkbox-primary checkbox-sm shrink-0"
                            checked={checked().has(profile.id)}
                            aria-label={`Select ${profile.name}`}
                            onChange={() => toggleChecked(profile.id)}
                          />
                          <div class="min-w-0 flex-1">
                            <span class="block truncate text-sm font-medium" title={profile.name}>
                              {profile.name}
                            </span>
                            <span class="block text-xs text-base-content/50">
                              {profile.itemCount} endpoints
                            </span>
                          </div>
                          <label
                            class="flex shrink-0 cursor-pointer items-center gap-2"
                            title={
                              profile.sidebarVisible
                                ? "Visible in the sidebar — click to hide"
                                : "Hidden from the sidebar — click to show"
                            }
                          >
                            <span class="hidden text-xs text-base-content/50 lg:block">Sidebar</span>
                            <input
                              type="checkbox"
                              class="toggle toggle-primary toggle-sm"
                              checked={profile.sidebarVisible}
                              disabled={actions.busy()}
                              aria-label={`Show ${profile.name} in the sidebar`}
                              onChange={(event) =>
                                void actions.setSidebarVisible(
                                  profile.id,
                                  event.currentTarget.checked,
                                )
                              }
                            />
                          </label>
                        </li>
                      )}
                    </Show>
                  )}
                </For>
              </ul>
            </Show>

            <Show when={actions.error()}>
              <div class="alert alert-error py-2 text-sm" role="alert">
                <span class="break-all">{actions.error()}</span>
              </div>
            </Show>
          </Show>
        </div>
      </div>
    </section>
  );
};
