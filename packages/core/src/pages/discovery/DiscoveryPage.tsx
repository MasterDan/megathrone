import type { Component } from "solid-js";
import { For, Show, createMemo, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import {
  TbOutlineArrowLeft,
  TbOutlineCircleCheck,
  TbOutlinePlayerPlay,
  TbOutlinePlayerStop,
  TbOutlinePlus,
  TbOutlineRadar,
  TbOutlineRss,
  TbOutlineX,
} from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { Empty } from "@/components/common/Empty";
import { ComboBox } from "@/components/common/daisy-ui/forms/ComboBox";
import { useIsMobileUi } from "@/contexts/uiVariant";
import { useDiscovery } from "@/hooks/data/useDiscovery";
import type {
  DiscoverySourceProgress,
} from "@/hooks/data/useDiscovery";
import { SourceGroupRow } from "@/pages/discovery/SourceGroupRow";
import type { SourceGroupEntry } from "@/pages/discovery/SourceGroupRow";
import { SourceRow } from "@/pages/discovery/SourceRow";
import { formatInterval } from "@/utils/time";
import type { DiscoverySource } from "@/types";

const INTERVALS = [30, 60, 360, 720, 1440];
const INTERVAL_ITEMS = [
  { value: 0, label: "Off" },
  ...INTERVALS.map((value) => ({ value, label: formatInterval(value) })),
];

/** One rendered catalog row: a standalone source, or a merge group
 *  collapsed into its single shared-profile row. */
type SourceEntry = { kind: "single"; source: DiscoverySource } | SourceGroupEntry;

/** Collapses a source list into display entries — group members merge
 *  into one entry placed at the group's first member, everything else
 *  stays a single row, the order is preserved. */
const groupSources = (list: DiscoverySource[]): SourceEntry[] => {
  const entries: SourceEntry[] = [];
  const byGroup = new Map<string, SourceGroupEntry>();
  for (const source of list) {
    if (source.mergeGroup === null) {
      entries.push({ kind: "single", source });
      continue;
    }
    const existing = byGroup.get(source.mergeGroup);
    if (existing) {
      existing.members.push(source);
    } else {
      const entry: SourceGroupEntry = {
        kind: "group",
        group: source.mergeGroup,
        owner: source,
        members: [source],
      };
      byGroup.set(source.mergeGroup, entry);
      entries.push(entry);
    }
  }
  return entries;
};

/** The Discovery page: an editable catalog of public subscription
 *  sources and a Run button that fetches every enabled one into its own
 *  profile (optionally latency-testing each afterwards), with live
 *  per-source progress and cancellation. */
export const DiscoveryPage: Component = () => {
  const discovery = useDiscovery();
  const isMobile = useIsMobileUi();

  const [newUrl, setNewUrl] = createSignal("");
  const [newName, setNewName] = createSignal("");
  const [newGroup, setNewGroup] = createSignal("");
  const [formError, setFormError] = createSignal<string | null>(null);

  const sources = createMemo(() => discovery.sources() ?? []);
  const enabledSources = createMemo(() => sources().filter((source) => source.enabled));
  const entries = createMemo(() => groupSources(sources()));
  const enabledEntries = createMemo(() => groupSources(enabledSources()));
  const running = discovery.running;
  const runStatus = discovery.runStatus;
  const selectedInterval = () =>
    INTERVAL_ITEMS.find((item) => item.value === (discovery.autoUpdateMinutes() ?? 0));

  const start = () => {
    if (running()) {
      return;
    }
    void discovery.run(discovery.testAfter());
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
    const group = newGroup().trim();
    void discovery
      .addSource(url, name ? name : undefined, group ? group : undefined)
      .then((ok) => {
        if (ok) {
          setNewUrl("");
          setNewName("");
          setNewGroup("");
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

  /** The run-progress row state of an entry: singles take their own
   *  per-URL outcome, a group its GroupDone (or pending while any member
   *  is in flight). */
  const entryProgress = (entry: SourceEntry): DiscoverySourceProgress | undefined => {
    if (entry.kind === "single") {
      return discovery.sourceProgress()[entry.source.id];
    }
    const finished = discovery.groupProgress()[entry.group];
    if (finished) {
      return finished.ok
        ? { status: "done", itemCount: finished.itemCount, error: null }
        : { status: "failed", itemCount: 0, error: finished.error };
    }
    const queued = entry.members.every(
      (member) => discovery.sourceProgress()[member.id] === undefined,
    );
    return queued ? undefined : { status: "pending", itemCount: 0, error: null };
  };

  const renderEntry = (entry: SourceEntry) => {
    const progress = runStatus() === null ? undefined : entryProgress(entry);
    if (entry.kind === "group") {
      return (
        <SourceGroupRow
          entry={entry}
          busy={discovery.busy()}
          progress={progress}
          onUpdateMember={discovery.updateSource}
          onAddMember={(url) => discovery.addSource(url, undefined, entry.group)}
          onDeleteMember={discovery.deleteSource}
          onDeleteGroup={discovery.deleteGroup}
        />
      );
    }
    return (
      <SourceRow
        source={entry.source}
        busy={discovery.busy()}
        progress={progress}
        onUpdate={discovery.updateSource}
        onDelete={discovery.deleteSource}
      />
    );
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
                {entries().length} sources · {enabledEntries().length} in runs
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
                  checked={discovery.testAfter()}
                  disabled={running()}
                  onChange={(event) => void discovery.saveTestAfter(event.currentTarget.checked)}
                />
              </label>
              <div
                class="flex items-center gap-2 text-sm text-base-content/70"
                title="Re-run Discovery automatically while the app is open"
              >
                <span class="whitespace-nowrap">Auto-update</span>
                <div class="w-36">
                  <ComboBox
                    search={(query) => {
                      const needle = query.trim().toLowerCase();
                      return needle
                        ? INTERVAL_ITEMS.filter((item) =>
                            item.label.toLowerCase().includes(needle),
                          )
                        : INTERVAL_ITEMS;
                    }}
                    getKey={(item) => item.value}
                    displayValue={(item) => item.label}
                    placeholder="Interval"
                    disabled={running() || discovery.savingSettings()}
                    value={selectedInterval()}
                    onChange={(item) => {
                      if (item) {
                        void discovery.saveAutoUpdate(item.value === 0 ? null : item.value);
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
              </div>
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
              <For each={entries()}>{(entry) => renderEntry(entry)}</For>
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
            <input
              type="text"
              class="input input-bordered input-sm w-full"
              placeholder="merge group (optional — existing or new)"
              value={newGroup()}
              disabled={discovery.busy()}
              onInput={(event) => setNewGroup(event.currentTarget.value)}
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
