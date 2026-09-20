import type { Accessor, Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";
import { TbOutlineRefresh, TbOutlineTrash } from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { Modal } from "@/components/common/daisy-ui/Modal";
import { ComboBox } from "@/components/common/daisy-ui/forms/ComboBox";
import { useProfileActions } from "@/hooks/data/useProfileActions";
import { useProfileUpdating } from "@/hooks/data/useProfileUpdating";
import { formatInterval, formatRelativeTime } from "@/utils/time";
import type { ProfileSummary } from "@/types";

const INTERVALS = [30, 60, 360, 720, 1440];
const INTERVAL_ITEMS = [
  { value: 0, label: "Off" },
  ...INTERVALS.map((value) => ({ value, label: formatInterval(value) })),
];

function sourceLabel(profile: ProfileSummary) {
  return profile.sourceUrl ?? profile.sourcePath ?? "";
}

interface Props {
  profile: ProfileSummary | null;
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
  onDone: () => void;
}

export const EditProfileModal: Component<Props> = (props) => {
  // The list refresh arrives via the profiles-changed event; close explicitly.
  const { busy, error, rename, remove, setAutoUpdate, update } = useProfileActions(() => {});
  const { updating: remotelyUpdating } = useProfileUpdating(() => props.profile?.id ?? -1);

  // true while any update of this profile runs — clicked or scheduled
  const updating = () => busy() || remotelyUpdating();

  const [name, setName] = createSignal("");
  const [minutes, setMinutes] = createSignal(0);
  const [confirmDelete, setConfirmDelete] = createSignal(false);

  // The modal stays mounted for exit animations, so drafts reset on open —
  // the profile may have been edited or refreshed since the previous one.
  createEffect(() => {
    const profile = props.profile;
    if (props.opened() && profile) {
      setName(profile.name);
      setMinutes(profile.autoUpdateMinutes ?? 0);
      setConfirmDelete(false);
    }
  });

  const hasSource = () =>
    Boolean(props.profile?.sourceUrl || props.profile?.sourcePath);
  const selectedInterval = () =>
    INTERVAL_ITEMS.find((item) => item.value === minutes());

  const save = async () => {
    const profile = props.profile;
    const trimmed = name().trim();
    if (!profile || !trimmed) {
      return;
    }
    if (trimmed !== profile.name) {
      if (!(await rename(profile.id, trimmed))) {
        return;
      }
    }
    const next = hasSource() ? minutes() || null : null;
    if (next !== (profile.autoUpdateMinutes ?? null)) {
      if (!(await setAutoUpdate(profile.id, next))) {
        return;
      }
    }
    props.onDone();
  };

  const updateNow = async () => {
    if (props.profile && (await update(props.profile.id))) {
      props.onDone();
    }
  };

  const confirmRemove = async () => {
    if (props.profile && (await remove(props.profile.id))) {
      props.onDone();
    }
  };

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title="Edit profile"
      class="max-w-lg"
      actions={
        <div class="flex w-full flex-wrap items-center justify-between gap-2">
          <Show
            when={!confirmDelete()}
            fallback={
              <Button ghost size="sm" disabled={busy()} onClick={() => setConfirmDelete(false)}>
                No
              </Button>
            }
          >
            <Button variant="error" size="sm" class="gap-1" disabled={busy()} onClick={() => setConfirmDelete(true)}>
              <TbOutlineTrash size={16} />
              Delete profile
            </Button>
          </Show>
          <div class="flex gap-2">
            <Show when={confirmDelete()}>
              <Button variant="error" size="sm" class="gap-1" disabled={busy()} onClick={() => void confirmRemove()}>
                <Show when={!busy()} fallback={<span class="loading loading-spinner loading-xs" />}>
                  <TbOutlineTrash size={16} />
                </Show>
                Yes, delete
              </Button>
            </Show>
            <Button ghost disabled={busy()} onClick={() => props.setOpened(false)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              type="submit"
              form="edit-profile-form"
              disabled={busy() || !name().trim()}
            >
              Save
            </Button>
          </div>
        </div>
      }
    >
      <form
        id="edit-profile-form"
        class="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          if (!busy()) {
            void save();
          }
        }}
      >
        <Show when={props.profile} keyed>
          {(profile) => (
            <p class="truncate text-xs text-base-content/50" title={sourceLabel(profile)}>
              {sourceLabel(profile) || "pasted text"}
            </p>
          )}
        </Show>

        <label class="form-control">
          <span class="label-text mb-1 block text-sm">Name</span>
          <input
            type="text"
            class="input input-bordered w-full"
            value={name()}
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </label>

        <div class="form-control">
          <span class="label-text mb-1 block text-sm">Auto-update interval</span>
          <div class="flex gap-2">
            <div class="min-w-0 flex-1">
              <ComboBox
                search={(query) => {
                  const needle = query.trim().toLowerCase();
                  return needle
                    ? INTERVAL_ITEMS.filter((item) => item.label.toLowerCase().includes(needle))
                    : INTERVAL_ITEMS;
                }}
                getKey={(item) => item.value}
                displayValue={(item) => item.label}
                placeholder="Interval"
                disabled={!hasSource() || busy()}
                value={selectedInterval()}
                onChange={(item) => {
                  if (item) {
                    setMinutes(item.value);
                  }
                }}
              >
                {(item, api) => (
                  <button
                    type="button"
                    class="w-full rounded-lg px-3 py-2 text-left"
                    classList={{ "bg-base-200": api.selected() }}
                    onClick={api.select}
                  >
                    {item.label}
                  </button>
                )}
              </ComboBox>
            </div>
            <Button
              outline
              class="shrink-0 gap-1"
              disabled={!hasSource() || updating()}
              title={
                updating()
                  ? "Updating…"
                  : hasSource()
                    ? "Fetch the profile now"
                    : "No source to update from"
              }
              onClick={() => void updateNow()}
            >
              <Show when={!updating()} fallback={<span class="loading loading-spinner loading-xs" />}>
                <TbOutlineRefresh size={16} />
              </Show>
              Update now
            </Button>
          </div>
        </div>

        <Show when={props.profile} keyed>
          {(profile) => (
            <p class="text-xs text-base-content/50">
              Last updated: {formatRelativeTime(profile.lastFetchedAt)}. While the app is closed
              time keeps running, so an elapsed interval triggers a refresh on the next launch.
            </p>
          )}
        </Show>

        <div class="divider my-0" />

        <Show when={error()}>
          <div class="alert alert-error py-2 text-sm">
            <span class="break-all">{error()}</span>
          </div>
        </Show>
      </form>
    </Modal>
  );
};
