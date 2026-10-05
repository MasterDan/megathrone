import type { Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import {
  TbOutlineAlertTriangle,
  TbOutlineCircleCheck,
  TbOutlineClock,
  TbOutlineExternalLink,
  TbOutlinePencil,
  TbOutlineStack2,
  TbOutlineTrash,
  TbOutlineX,
} from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { Modal } from "@/components/common/daisy-ui/Modal";
import type { DiscoverySourceProgress } from "@/hooks/data/useDiscovery";
import { ProgressOutcome } from "@/pages/discovery/ProgressOutcome";
import type { DiscoverySource } from "@/types";
import { formatRelativeTime } from "@/utils/time";

export const DeleteSourceModal: Component<{
  source: DiscoverySource;
  opened: () => boolean;
  setOpened: (value: boolean) => void;
  busy: () => boolean;
  onDelete: (deleteProfile: boolean) => void;
}> = (props) => {
  const [deleteProfile, setDeleteProfile] = createSignal(false);

  createEffect(() => {
    if (props.opened()) {
      setDeleteProfile(props.source.profileId !== null);
    }
  });

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title="Delete source"
      class="max-w-md"
      actions={
        <>
          <Button ghost disabled={props.busy()} onClick={() => props.setOpened(false)}>
            Cancel
          </Button>
          <Button
            variant="error"
            disabled={props.busy()}
            onClick={() => props.onDelete(deleteProfile())}
          >
            Delete
          </Button>
        </>
      }
    >
      <div class="space-y-3">
        <p class="text-sm">
          Delete <span class="font-semibold">{props.source.name}</span> from the Discovery
          catalog?
        </p>
        <Show when={props.source.mergeGroup !== null}>
          <p class="text-xs text-base-content/50">
            This source is part of a merge group — its profile is shared with the
            other members and is only deleted when the last one goes.
          </p>
        </Show>
        <Show
          when={props.source.profileId !== null}
          fallback={
            <p class="text-xs text-base-content/50">
              No profile is linked to this source yet.
            </p>
          }
        >
          <label class="flex cursor-pointer items-center gap-2 text-sm">
            <input
              type="checkbox"
              class="checkbox checkbox-sm checkbox-error"
              checked={deleteProfile()}
              onChange={(event) => setDeleteProfile(event.currentTarget.checked)}
            />
            Also delete the linked profile and its endpoints
          </label>
        </Show>
      </div>
    </Modal>
  );
};

/** One standalone (ungrouped) source row: inline rename, run toggle,
 *  delete with its confirm modal, last-run outcome and the live run
 *  progress chip while a run is on screen. */
export const SourceRow: Component<{
  source: DiscoverySource;
  busy: boolean;
  progress?: DiscoverySourceProgress | undefined;
  onUpdate: (
    id: number,
    changes: { url?: string; name?: string; enabled?: boolean },
  ) => Promise<boolean>;
  onDelete: (id: number, deleteProfile: boolean) => Promise<boolean>;
}> = (props) => {
  const [editing, setEditing] = createSignal(false);
  const [draft, setDraft] = createSignal(props.source.name);
  const [confirmOpened, setConfirmOpened] = createSignal(false);

  // drafts follow the stored name unless the user is mid-edit
  createEffect(() => {
    setDraft((current) => (current === props.source.name ? current : props.source.name));
  });

  const saveName = () => {
    const name = draft().trim();
    if (!name || props.busy || name === props.source.name) {
      setEditing(false);
      setDraft(props.source.name);
      return;
    }
    void props.onUpdate(props.source.id, { name }).then((ok) => {
      if (ok) {
        setEditing(false);
      }
    });
  };

  const remove = (deleteProfile: boolean) => {
    void props.onDelete(props.source.id, deleteProfile).then((ok) => {
      if (ok) {
        setConfirmOpened(false);
      }
    });
  };

  return (
    <li class="space-y-1.5 rounded-xl bg-base-200/60 p-3">
      <div class="flex items-center gap-1.5">
        <div class="min-w-0 flex-1">
          <Show
            when={!editing()}
            fallback={
              <div class="flex items-center gap-1.5">
                <input
                  type="text"
                  class="input input-bordered input-sm min-w-0 flex-1"
                  value={draft()}
                  disabled={props.busy}
                  onInput={(event) => setDraft(event.currentTarget.value)}
                  onBlur={saveName}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      saveName();
                    }
                    if (event.key === "Escape") {
                      setDraft(props.source.name);
                      setEditing(false);
                    }
                  }}
                />
                <button
                  type="button"
                  class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-success transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
                  title="Save name"
                  aria-label="Save name"
                  disabled={props.busy || !draft().trim()}
                  onClick={saveName}
                >
                  <TbOutlineCircleCheck size={15} />
                </button>
                <button
                  type="button"
                  class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
                  title="Discard changes"
                  aria-label="Discard changes"
                  disabled={props.busy}
                  onClick={() => {
                    setDraft(props.source.name);
                    setEditing(false);
                  }}
                >
                  <TbOutlineX size={15} />
                </button>
              </div>
            }
          >
            <div class="flex min-w-0 items-center gap-1.5">
              <button
                type="button"
                class="min-w-0 cursor-pointer truncate text-left text-sm font-medium underline-offset-2 transition-colors hover:underline disabled:pointer-events-none disabled:opacity-40"
                title="Rename source"
                disabled={props.busy}
                onClick={() => setEditing(true)}
              >
                {props.source.name}
              </button>
              <Show when={props.source.mergeGroup !== null}>
                <span
                  class="flex shrink-0 items-center gap-0.5 rounded-full bg-secondary/20 px-1.5 py-0.5 text-[10px] font-medium whitespace-nowrap text-secondary"
                  title={`Fetches into one shared profile with the "${props.source.mergeGroup}" merge group`}
                >
                  <TbOutlineStack2 size={11} />
                  merged
                </span>
              </Show>
              <Show when={props.source.profileId !== null}>
                <A
                  href={`/profiles/${props.source.profileId}`}
                  class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-primary transition-colors hover:bg-base-content/20"
                  title="Open the linked profile"
                  aria-label="Open the linked profile"
                >
                  <TbOutlineExternalLink size={14} />
                </A>
              </Show>
            </div>
          </Show>
          <p
            class="truncate font-mono text-xs text-base-content/50"
            title={props.source.url}
          >
            {props.source.url}
          </p>
        </div>
        <Show when={!editing()}>
          <Show when={props.progress !== undefined}>
            <ProgressOutcome progress={props.progress} />
          </Show>
          <input
            type="checkbox"
            class="toggle toggle-primary toggle-sm shrink-0"
            aria-label="Include this source in runs"
            title={
              props.source.enabled
                ? "Included in runs — click to skip"
                : "Skipped in runs — click to include"
            }
            checked={props.source.enabled}
            disabled={props.busy}
            onChange={(event) =>
              void props.onUpdate(props.source.id, {
                enabled: event.currentTarget.checked,
              })
            }
          />
          <button
            type="button"
            class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
            title="Rename source"
            aria-label="Rename source"
            disabled={props.busy}
            onClick={() => setEditing(true)}
          >
            <TbOutlinePencil size={16} />
          </button>
          <button
            type="button"
            class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-error transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
            title="Delete source"
            aria-label="Delete source"
            disabled={props.busy}
            onClick={() => setConfirmOpened(true)}
          >
            <TbOutlineTrash size={16} />
          </button>
        </Show>
      </div>

      <Show
        when={props.source.lastError}
        fallback={
          <Show when={props.source.lastItemCount !== null}>
            <p class="flex items-center gap-1 text-xs text-base-content/50">
              <TbOutlineCircleCheck size={12} class="shrink-0 text-success" />
              {props.source.lastItemCount} endpoints
              <Show when={props.source.lastRunAt}>
                {(at) => (
                  <>
                    <TbOutlineClock size={12} class="ml-1 shrink-0" />
                    {formatRelativeTime(at())}
                  </>
                )}
              </Show>
            </p>
          </Show>
        }
      >
        {(lastError) => (
          <p class="flex items-start gap-1 text-xs text-error" title={lastError()}>
            <TbOutlineAlertTriangle size={12} class="mt-0.5 shrink-0" />
            <span class="line-clamp-2 break-all">{lastError()}</span>
            <Show when={props.source.lastRunAt}>
              {(at) => (
                <span class="ml-auto shrink-0 whitespace-nowrap text-base-content/40">
                  {formatRelativeTime(at())}
                </span>
              )}
            </Show>
          </p>
        )}
      </Show>

      <DeleteSourceModal
        source={props.source}
        opened={confirmOpened}
        setOpened={setConfirmOpened}
        busy={() => props.busy}
        onDelete={remove}
      />
    </li>
  );
};
