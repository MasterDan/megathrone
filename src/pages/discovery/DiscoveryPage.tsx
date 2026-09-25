import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import {
  TbOutlineAlertTriangle,
  TbOutlineArrowLeft,
  TbOutlineCircleCheck,
  TbOutlineCircleX,
  TbOutlineClock,
  TbOutlineExternalLink,
  TbOutlinePencil,
  TbOutlinePlayerPlay,
  TbOutlinePlayerStop,
  TbOutlinePlus,
  TbOutlineRadar,
  TbOutlineRss,
  TbOutlineTrash,
  TbOutlineX,
} from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { Empty } from "@/components/common/Empty";
import { Modal } from "@/components/common/daisy-ui/Modal";
import { useIsMobileUi } from "@/contexts/uiVariant";
import { useDiscovery } from "@/hooks/data/useDiscovery";
import type { DiscoverySourceProgress } from "@/hooks/data/useDiscovery";
import { formatRelativeTime } from "@/utils/time";
import type { DiscoverySource } from "@/types";

const DeleteSourceModal: Component<{
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

const SourceRow: Component<{
  source: DiscoverySource;
  busy: boolean;
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

const ProgressOutcome: Component<{
  progress: DiscoverySourceProgress | undefined;
}> = (props) => (
  <Show
    when={props.progress}
    fallback={<span class="text-xs whitespace-nowrap text-base-content/40">queued</span>}
    keyed
  >
    {(entry) => (
      <Show
        when={entry.status === "pending"}
        fallback={
          <Show
            when={entry.status === "done"}
            fallback={
              <span
                class="flex min-w-0 items-center gap-1 text-xs text-error"
                title={entry.error ?? undefined}
              >
                <TbOutlineCircleX size={14} class="shrink-0" />
                <span class="truncate">{entry.error ?? "failed"}</span>
              </span>
            }
          >
            <span class="flex items-center gap-1 text-xs whitespace-nowrap text-success">
              <TbOutlineCircleCheck size={14} class="shrink-0" />
              {entry.itemCount} endpoints
            </span>
          </Show>
        }
      >
        <span class="loading loading-spinner loading-xs text-primary" />
      </Show>
    )}
  </Show>
);

/** The Discovery page: an editable catalog of public subscription
 *  sources and a Run button that fetches every enabled one into its own
 *  profile (optionally latency-testing each afterwards), with live
 *  per-source progress and cancellation. */
export const DiscoveryPage: Component = () => {
  const discovery = useDiscovery();
  const isMobile = useIsMobileUi();

  const [testAfter, setTestAfter] = createSignal(true);
  const [newUrl, setNewUrl] = createSignal("");
  const [newName, setNewName] = createSignal("");
  const [formError, setFormError] = createSignal<string | null>(null);

  const sources = createMemo(() => discovery.sources() ?? []);
  const enabledSources = createMemo(() => sources().filter((source) => source.enabled));
  const running = discovery.running;
  const runStatus = discovery.runStatus;

  const start = () => {
    if (running()) {
      return;
    }
    void discovery.run(testAfter());
  };

  const submitSource = () => {
    const url = newUrl().trim();
    if (!url) {
      setFormError("URL is required");
      return;
    }
    let parsed: URL;
    try {
      parsed = new URL(url);
    } catch {
      setFormError("Not a valid URL");
      return;
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      setFormError("Only http(s) URLs are supported");
      return;
    }
    setFormError(null);
    const name = newName().trim();
    void discovery.addSource(url, name ? name : undefined).then((ok) => {
      if (ok) {
        setNewUrl("");
        setNewName("");
      }
    });
  };

  const progressCaption = () => {
    const status = runStatus();
    if (status === null) {
      return "";
    }
    return status.phase === "testing" ? "testing" : `${status.done}/${status.total}`;
  };

  return (
    <div class="container mx-auto max-w-2xl space-y-4 p-4">
      <Show when={discovery.loading()}>
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner text-primary" />
        </div>
      </Show>

      <Show when={!discovery.loading()}>
        <div
          classList={{
            "sticky z-40 -mx-4 rounded-2xl border border-base-content/10 bg-base-100/60 px-4 py-2.5 shadow-sm backdrop-blur-md": true,
            "top-14": isMobile(),
            "top-0": !isMobile(),
          }}
        >
          <div class="flex items-center gap-2">
            <A
              href="/profiles"
              class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
              title="Back to profiles"
              aria-label="Back to profiles"
            >
              <TbOutlineArrowLeft size={16} />
            </A>
            <div class="min-w-0">
              <h1 class="truncate text-lg font-bold">Discovery</h1>
              <p class="truncate text-xs text-base-content/50">
                {sources().length} sources · {enabledSources().length} in runs
              </p>
            </div>
          </div>
        </div>

        <div class="card bg-base-content/5 shadow-sm">
          <div class="card-body gap-3 p-4">
            <div class="space-y-0.5">
              <h2 class="text-sm font-semibold">Run</h2>
              <p class="text-xs text-base-content/50">
                Fetch public proxy subscriptions and create a profile per source.
              </p>
            </div>

            <Show when={discovery.runError()}>
              {(message) => (
                <div class="alert alert-error py-2 text-sm" role="alert">
                  <span class="break-all">{message()}</span>
                  <button
                    type="button"
                    class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full transition-colors hover:bg-base-content/20"
                    aria-label="Dismiss error"
                    onClick={() => discovery.dismissRunError()}
                  >
                    <TbOutlineX size={12} />
                  </button>
                </div>
              )}
            </Show>

            <div class="flex flex-wrap items-center gap-3">
              <label
                class="flex cursor-pointer items-center gap-2 text-sm text-base-content/70"
                title="After fetching, run a latency scan on every created profile"
              >
                Test after
                <input
                  type="checkbox"
                  class="toggle toggle-sm toggle-primary"
                  checked={testAfter()}
                  disabled={running()}
                  onChange={(event) => setTestAfter(event.currentTarget.checked)}
                />
              </label>
              <div class="ml-auto flex items-center gap-2">
                <Show
                  when={running()}
                  fallback={
                    <Button
                      variant="primary"
                      size="sm"
                      disabled={enabledSources().length === 0}
                      title={
                        enabledSources().length === 0
                          ? "No enabled sources to fetch"
                          : "Fetch every enabled source"
                      }
                      onClick={start}
                    >
                      <TbOutlinePlayerPlay size={16} />
                      Run
                    </Button>
                  }
                >
                  <Button variant="primary" size="sm" disabled title="A run is in progress">
                    <span class="loading loading-spinner loading-xs" />
                    {progressCaption()}
                  </Button>
                  <Button
                    variant="error"
                    size="sm"
                    disabled={discovery.cancelling()}
                    onClick={() => void discovery.cancel()}
                  >
                    <TbOutlinePlayerStop size={16} />
                    <Show when={!discovery.cancelling()} fallback="Cancelling…">
                      Cancel
                    </Show>
                  </Button>
                </Show>
              </div>
            </div>

            <Show when={runStatus()?.phase === "testing"}>
              <div class="flex items-start gap-2 rounded-xl bg-base-200/60 px-3 py-2 text-sm">
                <span class="loading loading-spinner loading-xs mt-1 text-primary" />
                <div class="min-w-0">
                  Testing{" "}
                  <span class="font-medium">
                    {runStatus()?.testProfileName ?? "profile"}
                  </span>
                  <p class="text-xs text-base-content/50">
                    Latency scans run in the background — follow them on the profile
                    pages after closing this page.
                  </p>
                </div>
              </div>
            </Show>

            <Show when={discovery.lastFinished()}>
              {(summary) => (
                <div
                  class="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-sm"
                  role="status"
                >
                  <TbOutlineCircleCheck size={16} class="shrink-0 text-success" />
                  <span>
                    {summary().created} created, {summary().updated} updated,{" "}
                    {summary().failed} failed
                  </span>
                  <Show when={summary().cancelled}>
                    <span class="rounded-full bg-warning/20 px-2 py-0.5 text-xs font-medium text-warning">
                      cancelled
                    </span>
                  </Show>
                </div>
              )}
            </Show>
          </div>
        </div>

        <Show when={runStatus() !== null}>
          <section class="space-y-2">
            <h2 class="flex items-center gap-2 px-1 text-sm font-semibold text-base-content/70">
              <TbOutlineRadar size={16} class="text-base-content/50" />
              Run progress
            </h2>
            <ul class="space-y-1.5">
              <For each={enabledSources()}>
                {(source) => (
                  <li class="flex items-center gap-2 rounded-xl bg-base-200/60 px-3 py-2">
                    <span class="min-w-0 flex-1 truncate text-sm" title={source.name}>
                      {source.name}
                    </span>
                    <ProgressOutcome progress={discovery.sourceProgress()[source.id]} />
                  </li>
                )}
              </For>
            </ul>
          </section>
        </Show>

        <Show when={discovery.error()}>
          <div class="alert alert-error py-2 text-sm" role="alert">
            <span class="break-all">{discovery.error()}</span>
          </div>
        </Show>

        <Show
          when={sources().length > 0}
          fallback={
            <Empty
              icon={TbOutlineRadar}
              title="No sources yet"
              description="Add a public subscription URL below — each run fetches every enabled source into its own profile."
            />
          }
        >
          <section class="space-y-2">
            <h2 class="flex items-center gap-2 px-1 text-sm font-semibold text-base-content/70">
              <TbOutlineRss size={16} class="text-base-content/50" />
              Sources
            </h2>
            <ul class="space-y-2">
              <For each={sources()}>
                {(source) => (
                  <SourceRow
                    source={source}
                    busy={discovery.busy()}
                    onUpdate={discovery.updateSource}
                    onDelete={discovery.deleteSource}
                  />
                )}
              </For>
            </ul>
          </section>
        </Show>

        <div class="card bg-base-content/5 shadow-sm">
          <div class="card-body gap-2 p-4">
            <h2 class="flex items-center gap-2 text-sm font-semibold">
              <TbOutlinePlus size={16} class="text-base-content/50" />
              Add source
            </h2>
            <input
              type="text"
              class="input input-bordered input-sm w-full font-mono text-xs"
              placeholder="https://example.com/subscription.txt"
              value={newUrl()}
              disabled={discovery.busy()}
              classList={{ "input-error": formError() !== null }}
              onInput={(event) => {
                setNewUrl(event.currentTarget.value);
                setFormError(null);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  submitSource();
                }
              }}
            />
            <input
              type="text"
              class="input input-bordered input-sm w-full"
              placeholder="name (optional, defaults to Omega-N)"
              value={newName()}
              disabled={discovery.busy()}
              onInput={(event) => setNewName(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  submitSource();
                }
              }}
            />
            <div class="flex items-center justify-between gap-2">
              <Show when={formError()}>
                {(message) => (
                  <p class="text-xs text-error" role="alert">
                    {message()}
                  </p>
                )}
              </Show>
              <button
                type="button"
                class="ml-auto inline-flex h-8 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-primary/20 px-3 text-sm font-medium whitespace-nowrap text-primary transition-colors hover:bg-primary/30 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
                disabled={discovery.busy() || !newUrl().trim()}
                onClick={submitSource}
              >
                <TbOutlinePlus size={16} />
                Add
              </button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
};
