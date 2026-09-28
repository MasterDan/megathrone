import type { Component } from "solid-js";
import { For, Show, createSignal } from "solid-js";
import { useNavigate } from "@solidjs/router";
import {
  TbOutlineFolders,
  TbOutlinePlus,
  TbOutlineRadar,
  TbOutlineRefresh,
} from "solid-icons/tb";

import { Empty } from "@/components/common/Empty";
import { AddProfileModal } from "@/components/profiles/AddProfileModal";
import { EditProfileModal } from "@/components/profiles/EditProfileModal";
import { ProfileCard } from "@/components/profiles/ProfileCard";
import { useProfiles } from "@/hooks/data/useProfiles";
import type { ProfileSummary } from "@/types";

export const ProfilesList: Component = () => {
  const { profiles, loading, error, refetch } = useProfiles();
  const navigate = useNavigate();

  const [adding, setAdding] = createSignal(false);
  const [editing, setEditing] = createSignal<ProfileSummary | null>(null);

  const ImportFirstAction: Component = () => (
    <button
      type="button"
      class="inline-flex h-8 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-primary/20 px-3 text-sm font-medium whitespace-nowrap text-primary transition-colors hover:bg-primary/30 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
      onClick={() => setAdding(true)}
    >
      <TbOutlinePlus size={16} />
      Import your first profile
    </button>
  );

  return (
    <div class="space-y-6">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <h1 class="flex items-center gap-2 text-2xl font-bold">
          <TbOutlineFolders size={26} class="text-base-content/60" />
          Profiles
        </h1>
        <div class="flex gap-2">
          <button
            type="button"
            class="flex size-8 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
            title="Discovery"
            aria-label="Discovery"
            onClick={() => navigate("/discovery")}
          >
            <TbOutlineRadar size={18} />
          </button>
          <button
            type="button"
            class="flex size-8 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
            title="Refresh"
            onClick={() => void refetch()}
          >
            <TbOutlineRefresh size={18} />
          </button>
          <button
            type="button"
            class="inline-flex h-8 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-primary/20 px-3 text-sm font-medium whitespace-nowrap text-primary transition-colors hover:bg-primary/30 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
            onClick={() => setAdding(true)}
          >
            <TbOutlinePlus size={18} />
            Add profile
          </button>
        </div>
      </div>

      <Show when={error()}>
        <div class="alert alert-error">
          <span class="break-all">{String(error())}</span>
        </div>
      </Show>

      <Show when={loading()}>
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner text-primary" />
        </div>
      </Show>

      <Show when={!loading() && !error() && (profiles()?.length ?? 0) === 0}>
        <Empty
          icon={TbOutlineFolders}
          title="No profiles yet"
          description="Import a subscription by URL or from a text file to get started."
          action={ImportFirstAction}
        />
      </Show>

      <div class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
        <For each={profiles() ?? []}>
          {(profile) => <ProfileCard profile={profile} onEdit={() => setEditing(profile)} />}
        </For>
      </div>

      <AddProfileModal
        opened={adding}
        setOpened={setAdding}
        onDone={() => void refetch()}
      />

      <EditProfileModal
        profile={editing()}
        opened={() => editing() !== null}
        setOpened={(open) => {
          if (!open) {
            setEditing(null);
          }
        }}
        onDone={() => {
          setEditing(null);
          void refetch();
        }}
      />
    </div>
  );
};
