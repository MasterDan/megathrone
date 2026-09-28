import type { Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";
import {
  TbOutlineCircleCheck,
  TbOutlinePencil,
  TbOutlineX,
} from "solid-icons/tb";

import type { DiscoverySource } from "@/types";

export const GroupMemberRow: Component<{
  member: DiscoverySource;
  isOwner: boolean;
  busy: boolean;
  onUpdate: (
    id: number,
    changes: { url?: string; name?: string; enabled?: boolean },
  ) => Promise<boolean>;
  onRemove: (member: DiscoverySource) => void;
}> = (props) => {
  const [editingUrl, setEditingUrl] = createSignal(false);
  const [urlDraft, setUrlDraft] = createSignal(props.member.url);
  const [editingName, setEditingName] = createSignal(false);
  const [nameDraft, setNameDraft] = createSignal(props.member.name);

  createEffect(() => {
    setUrlDraft((current) =>
      current === props.member.url ? current : props.member.url,
    );
  });
  createEffect(() => {
    setNameDraft((current) =>
      current === props.member.name ? current : props.member.name,
    );
  });

  const saveUrl = () => {
    const url = urlDraft().trim();
    if (!url || props.busy || url === props.member.url) {
      setEditingUrl(false);
      setUrlDraft(props.member.url);
      return;
    }
    void props.onUpdate(props.member.id, { url }).then((ok) => {
      if (ok) {
        setEditingUrl(false);
      }
    });
  };

  const saveName = () => {
    const name = nameDraft().trim();
    if (!name || props.busy || name === props.member.name) {
      setEditingName(false);
      setNameDraft(props.member.name);
      return;
    }
    void props.onUpdate(props.member.id, { name }).then((ok) => {
      if (ok) {
        setEditingName(false);
      }
    });
  };

  return (
    <li class="space-y-1 rounded-lg bg-base-200/80 px-2.5 py-1.5">
      <Show when={props.isOwner}>
        <Show
          when={!editingName()}
          fallback={
            <div class="flex items-center gap-1.5">
              <input
                type="text"
                class="input input-bordered input-xs min-w-0 flex-1"
                value={nameDraft()}
                disabled={props.busy}
                onInput={(event) => setNameDraft(event.currentTarget.value)}
                onBlur={saveName}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    saveName();
                  }
                  if (event.key === "Escape") {
                    setNameDraft(props.member.name);
                    setEditingName(false);
                  }
                }}
              />
              <button
                type="button"
                class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-success transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
                title="Save name"
                aria-label="Save name"
                disabled={props.busy || !nameDraft().trim()}
                onClick={saveName}
              >
                <TbOutlineCircleCheck size={13} />
              </button>
              <button
                type="button"
                class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
                title="Discard changes"
                aria-label="Discard changes"
                disabled={props.busy}
                onClick={() => {
                  setNameDraft(props.member.name);
                  setEditingName(false);
                }}
              >
                <TbOutlineX size={13} />
              </button>
            </div>
          }
        >
          <button
            type="button"
            class="block max-w-full cursor-pointer truncate text-left text-xs underline-offset-2 transition-colors hover:underline disabled:pointer-events-none disabled:opacity-40"
            title="Rename the group owner"
            disabled={props.busy}
            onClick={() => setEditingName(true)}
          >
            {props.member.name}
          </button>
        </Show>
      </Show>

      <div class="flex items-center gap-2">
        <Show
          when={!editingUrl()}
          fallback={
            <div class="flex min-w-0 flex-1 items-center gap-1.5">
              <input
                type="text"
                class="input input-bordered input-xs min-w-0 flex-1 font-mono"
                value={urlDraft()}
                disabled={props.busy}
                onInput={(event) => setUrlDraft(event.currentTarget.value)}
                onBlur={saveUrl}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    saveUrl();
                  }
                  if (event.key === "Escape") {
                    setUrlDraft(props.member.url);
                    setEditingUrl(false);
                  }
                }}
              />
              <button
                type="button"
                class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-success transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
                title="Save URL"
                aria-label="Save URL"
                disabled={props.busy || !urlDraft().trim()}
                onClick={saveUrl}
              >
                <TbOutlineCircleCheck size={13} />
              </button>
              <button
                type="button"
                class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
                title="Discard changes"
                aria-label="Discard changes"
                disabled={props.busy}
                onClick={() => {
                  setUrlDraft(props.member.url);
                  setEditingUrl(false);
                }}
              >
                <TbOutlineX size={13} />
              </button>
            </div>
          }
        >
          <span
            class="min-w-0 flex-1 truncate font-mono text-xs text-base-content/60"
            title={props.member.url}
          >
            {props.member.url}
          </span>
        </Show>
        <Show when={!editingUrl()}>
          <Show
            when={props.member.lastError}
            fallback={
              <span class="text-xs whitespace-nowrap text-base-content/40">
                {props.member.lastItemCount ?? "—"}
              </span>
            }
          >
            {(error) => (
              <span class="max-w-40 truncate text-xs text-error" title={error()}>
                {error()}
              </span>
            )}
          </Show>
          <input
            type="checkbox"
            class="toggle toggle-primary toggle-xs shrink-0"
            aria-label="Include this URL in runs"
            title={
              props.member.enabled
                ? "Included in runs — click to skip"
                : "Skipped in runs — click to include"
            }
            checked={props.member.enabled}
            disabled={props.busy}
            onChange={(event) =>
              void props.onUpdate(props.member.id, {
                enabled: event.currentTarget.checked,
              })
            }
          />
          <button
            type="button"
            class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full text-base-content/60 transition-colors hover:bg-base-content/10 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
            title="Edit URL"
            aria-label="Edit URL"
            disabled={props.busy}
            onClick={() => setEditingUrl(true)}
          >
            <TbOutlinePencil size={13} />
          </button>
          <button
            type="button"
            class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full text-error transition-colors hover:bg-error/10 disabled:pointer-events-none disabled:opacity-40"
            title="Remove this URL from the group"
            aria-label="Remove this URL from the group"
            disabled={props.busy}
            onClick={() => props.onRemove(props.member)}
          >
            <TbOutlineX size={13} />
          </button>
        </Show>
      </div>
    </li>
  );
};
