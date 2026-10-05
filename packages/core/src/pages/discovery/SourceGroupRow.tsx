import type { Component } from "solid-js";
import { For, Show, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import {
  TbOutlineAlertTriangle,
  TbOutlineChevronDown,
  TbOutlineChevronRight,
  TbOutlineCircleCheck,
  TbOutlineClock,
  TbOutlineExternalLink,
  TbOutlinePlus,
  TbOutlineStack2,
  TbOutlineTrash,
} from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { Modal } from "@/components/common/daisy-ui/Modal";
import type { DiscoverySourceProgress } from "@/hooks/data/useDiscovery";
import { GroupMemberRow } from "@/pages/discovery/GroupMemberRow";
import { ProgressOutcome } from "@/pages/discovery/ProgressOutcome";
import type { DiscoverySource } from "@/types";
import { formatRelativeTime } from "@/utils/time";

/** A merge group rendered as ONE catalog row: the members share a single
 *  profile, the per-URL toggles/errors live in the expandable sub-list. */
export interface SourceGroupEntry {
  kind: "group";
  group: string;
  owner: DiscoverySource;
  members: DiscoverySource[];
}

export const SourceGroupRow: Component<{
  entry: SourceGroupEntry;
  busy: boolean;
  progress?: DiscoverySourceProgress | undefined;
  onUpdateMember: (
    id: number,
    changes: { url?: string; name?: string; enabled?: boolean },
  ) => Promise<boolean>;
  onAddMember: (url: string) => Promise<boolean>;
  onDeleteMember: (id: number, deleteProfile: boolean) => Promise<boolean>;
  onDeleteGroup: (group: string) => Promise<boolean>;
}> = (props) => {
  const [expanded, setExpanded] = createSignal(false);
  const [confirmOpened, setConfirmOpened] = createSignal(false);
  const [newMemberUrl, setNewMemberUrl] = createSignal("");
  const [memberFormError, setMemberFormError] = createSignal<string | null>(null);

  const members = () => props.entry.members;
  const allEnabled = () => members().every((member) => member.enabled);
  const profileId = () =>
    members().find((member) => member.profileId !== null)?.profileId ?? null;
  const errorMember = () => members().find((member) => member.lastError) ?? null;
  const totalItems = () =>
    members().reduce((sum, member) => sum + (member.lastItemCount ?? 0), 0);
  const lastRunAt = () =>
    members().reduce<string | null>((latest, member) => {
      if (member.lastRunAt && (latest === null || member.lastRunAt > latest)) {
        return member.lastRunAt;
      }
      return latest;
    }, null);

  const setGroupEnabled = (enabled: boolean) => {
    for (const member of members()) {
      if (member.enabled !== enabled) {
        void props.onUpdateMember(member.id, { enabled });
      }
    }
  };

  const removeGroup = () => {
    void props.onDeleteGroup(props.entry.group).then((ok) => {
      if (ok) {
        setConfirmOpened(false);
      }
    });
  };

  // the shared profile survives member deletes until the last one goes
  const removeMember = (member: DiscoverySource) => {
    void props.onDeleteMember(member.id, members().length === 1);
  };

  const submitMember = () => {
    const url = newMemberUrl().trim();
    if (!url) {
      setMemberFormError("URL is required");
      return;
    }
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      setMemberFormError("Not a valid URL");
      return;
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      setMemberFormError("Only http(s) URLs are supported");
      return;
    }
    setMemberFormError(null);
    void props.onAddMember(url).then((ok) => {
      if (ok) {
        setNewMemberUrl("");
      }
    });
  };

  return (
    <li class="space-y-1.5 rounded-xl bg-base-200/60 p-3">
      <div class="flex items-center gap-1.5">
        <button
          type="button"
          class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
          title={expanded() ? "Collapse the merged URLs" : "Expand the merged URLs"}
          aria-label={expanded() ? "Collapse the merged URLs" : "Expand the merged URLs"}
          onClick={() => setExpanded((current) => !current)}
        >
          <Show when={expanded()} fallback={<TbOutlineChevronRight size={15} />}>
            <TbOutlineChevronDown size={15} />
          </Show>
        </button>
        <div class="min-w-0 flex-1">
          <div class="flex min-w-0 items-center gap-1.5">
            <span class="min-w-0 truncate text-sm font-medium">
              {props.entry.owner.name}
            </span>
            <span
              class="flex shrink-0 items-center gap-0.5 rounded-full bg-secondary/20 px-1.5 py-0.5 text-[10px] font-medium whitespace-nowrap text-secondary"
              title={`Fetches into one shared profile from ${members().length} merged sources`}
            >
              <TbOutlineStack2 size={11} />×{members().length}
            </span>
            <Show when={profileId() !== null}>
              <A
                href={`/profiles/${profileId()}`}
                class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-primary transition-colors hover:bg-base-content/20"
                title="Open the shared profile"
                aria-label="Open the shared profile"
              >
                <TbOutlineExternalLink size={14} />
              </A>
            </Show>
          </div>
          <p class="truncate text-xs text-base-content/50">
            one shared profile · {members().length} merged sources
          </p>
        </div>
        <Show when={props.progress !== undefined}>
          <ProgressOutcome progress={props.progress} />
        </Show>
        <input
          type="checkbox"
          class="toggle toggle-primary toggle-sm shrink-0"
          aria-label="Include this group in runs"
          title={
            allEnabled()
              ? "Included in runs — click to skip"
              : "Some or all sources skipped — click to include all"
          }
          checked={allEnabled()}
          disabled={props.busy}
          onChange={(event) => setGroupEnabled(event.currentTarget.checked)}
        />
        <button
          type="button"
          class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
          title="Delete group"
          aria-label="Delete group"
          disabled={props.busy}
          onClick={() => setConfirmOpened(true)}
        >
          <TbOutlineTrash size={16} />
        </button>
      </div>

      <Show
        when={errorMember()?.lastError}
        fallback={
          <Show when={totalItems() > 0}>
            <p class="flex items-center gap-1 text-xs text-base-content/50">
              <TbOutlineCircleCheck size={12} class="shrink-0 text-success" />
              {totalItems()} endpoints
              <Show when={lastRunAt()}>
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
            <Show when={lastRunAt()}>
              {(at) => (
                <span class="ml-auto shrink-0 whitespace-nowrap text-base-content/40">
                  {formatRelativeTime(at())}
                </span>
              )}
            </Show>
          </p>
        )}
      </Show>

      <Show when={expanded()}>
        <ul class="space-y-1">
          <For each={members()}>
            {(member, index) => (
              <GroupMemberRow
                member={member}
                isOwner={index() === 0}
                busy={props.busy}
                onUpdate={props.onUpdateMember}
                onRemove={removeMember}
              />
            )}
          </For>
          <li class="rounded-lg bg-base-200/80 px-2.5 py-1.5">
            <div class="flex items-center gap-1.5">
              <input
                type="text"
                class="input input-bordered input-xs min-w-0 flex-1 font-mono"
                placeholder="https://example.com/subscription.txt"
                value={newMemberUrl()}
                disabled={props.busy}
                classList={{ "input-error": memberFormError() !== null }}
                onInput={(event) => {
                  setNewMemberUrl(event.currentTarget.value);
                  setMemberFormError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    submitMember();
                  }
                }}
              />
              <button
                type="button"
                class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-primary/20 text-primary transition-colors hover:bg-primary/30 disabled:pointer-events-none disabled:opacity-40"
                title="Add URL to the group"
                aria-label="Add URL to the group"
                disabled={props.busy || !newMemberUrl().trim()}
                onClick={submitMember}
              >
                <TbOutlinePlus size={14} />
              </button>
            </div>
            <Show when={memberFormError()}>
              {(message) => (
                <p class="mt-1 text-xs text-error" role="alert">
                  {message()}
                </p>
              )}
            </Show>
          </li>
        </ul>
      </Show>

      <Modal
        opened={confirmOpened}
        setOpened={setConfirmOpened}
        title="Delete group"
        class="max-w-md"
        actions={
          <>
            <Button ghost disabled={props.busy} onClick={() => setConfirmOpened(false)}>
              Cancel
            </Button>
            <Button variant="error" disabled={props.busy} onClick={removeGroup}>
              Delete
            </Button>
          </>
        }
      >
        <p class="text-sm">
          Delete <span class="font-semibold">{props.entry.owner.name}</span> —{" "}
          {members().length} merged sources — from the Discovery catalog?
        </p>
        <p class="text-xs text-base-content/50">
          The shared profile and its endpoints go with it.
        </p>
      </Modal>
    </li>
  );
};
