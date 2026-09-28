import type { Component } from "solid-js";
import { createSignal, For, Show } from "solid-js";
import { A } from "@solidjs/router";
import {
  TbOutlineArrowLeft,
  TbOutlineChevronDown,
  TbOutlineCircleCheck,
  TbOutlineCpu,
  TbOutlineHelp,
  TbOutlineWorld,
  TbOutlineX,
} from "solid-icons/tb";

import { Empty } from "@/components/common/Empty";
import { TransitionCollapse } from "@/components/common/transitions/TransitionCollapse";
import { TrafficDock } from "@/components/home/TrafficDock";
import { useIsMobileUi } from "@/contexts/uiVariant";
import { useConnection } from "@/hooks/data/useConnection";
import { useUrlStats } from "@/hooks/data/useUrlStats";
import type { UrlStatEntry } from "@/types";

/** Tunnel badges reuse the chart palette (TrafficDock's buckets). */
const TUNNEL_BADGES: Record<UrlStatEntry["tunnel"], { label: string; color: string }> = {
  proxy: { label: "Proxy", color: "#8b5cf6" },
  dpi: { label: "DPI", color: "#10b981" },
  direct: { label: "Direct", color: "#0ea5e9" },
};

/** The ok/failed/total triple shared by every row (group headers included). */
const Counts: Component<{ ok: number; failed: number; requests: number }> = (props) => (
  <>
    <span
      class="flex w-12 shrink-0 items-center justify-end gap-1 text-xs tabular-nums text-success"
      classList={{ "opacity-40": props.ok === 0 }}
      title="Closed with a response"
    >
      <TbOutlineCircleCheck size={13} class="shrink-0" />
      {props.ok}
    </span>
    <span
      class="flex w-12 shrink-0 items-center justify-end gap-1 text-xs tabular-nums text-error"
      classList={{ "opacity-40": props.failed === 0 }}
      title="Closed without a response"
    >
      <TbOutlineX size={13} class="shrink-0" />
      {props.failed}
    </span>
    <span
      class="w-10 shrink-0 text-right text-sm font-semibold tabular-nums"
      title="Total requests, open ones included"
    >
      {props.requests}
    </span>
  </>
);

/** One summary row: the host that was reached, the tunnel it rode, the
 *  ok/failed split of its closed requests and the total (open ones
 *  included — they finalize once they close). */
const UrlStatRow: Component<{ entry: UrlStatEntry }> = (props) => {
  const badge = () => TUNNEL_BADGES[props.entry.tunnel];
  return (
    <li class="flex items-center gap-3 rounded-xl bg-base-200/60 px-3 py-2">
      <span class="min-w-0 flex-1 truncate font-mono text-xs" title={props.entry.host}>
        {props.entry.host}
      </span>
      <span
        class="badge badge-sm badge-outline shrink-0 whitespace-nowrap font-medium"
        style={{ color: badge().color, "border-color": badge().color }}
      >
        {badge().label}
      </span>
      <Counts ok={props.entry.ok} failed={props.entry.failed} requests={props.entry.requests} />
    </li>
  );
};

/** One process's rows folded together, plus the collapsed/expanded state
 *  key (the raw process string — "" groups everything sing-box could not
 *  attribute, shown as "Unknown process"). */
interface ProcessGroup {
  key: string;
  label: string;
  entries: UrlStatEntry[];
  requests: number;
  ok: number;
  failed: number;
}

/** One collapsible process section: the header carries the process name
 *  and its folded counts, the rows list the hosts it talked to. */
const ProcessGroupSection: Component<{
  group: ProcessGroup;
  collapsed: boolean;
  onToggle: () => void;
}> = (props) => (
  <section class="space-y-2">
    <button
      type="button"
      class="flex w-full cursor-pointer items-center gap-3 rounded-xl bg-base-content/10 px-3 py-2 text-left transition-colors hover:bg-base-content/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
      onClick={() => props.onToggle()}
      aria-expanded={!props.collapsed}
    >
      <TbOutlineChevronDown
        size={14}
        class="shrink-0 text-base-content/50 transition-transform duration-200"
        classList={{ "-rotate-90": props.collapsed }}
      />
      <Show
        when={props.group.key !== ""}
        fallback={<TbOutlineHelp size={14} class="shrink-0 text-base-content/40" />}
      >
        <TbOutlineCpu size={14} class="shrink-0 text-base-content/60" />
      </Show>
      <span class="min-w-0 flex-1 truncate font-mono text-xs font-semibold" title={props.group.key}>
        {props.group.label}
      </span>
      <Counts ok={props.group.ok} failed={props.group.failed} requests={props.group.requests} />
    </button>
    <TransitionCollapse>
      <Show when={!props.collapsed}>
        <ul class="ms-4 space-y-2">
          <For each={props.group.entries}>{(entry) => <UrlStatRow entry={entry} />}</For>
        </ul>
      </Show>
    </TransitionCollapse>
  </section>
);

/** Session statistics: the per-URL request summary on top, grouped by the
 *  process that made each request (each group folds to its header), and
 *  the same live traffic charts as on Home at the bottom. Opened by
 *  clicking the traffic dock on Home. The page fills the viewport like
 *  Home does, so the dock keeps its full-window width and sits flush at
 *  the window bottom; the summary scrolls above it. */
export const Stats: Component = () => {
  const stats = useUrlStats();
  const connection = useConnection();
  const isMobile = useIsMobileUi();
  const connected = () => connection.snapshot()?.connected ?? false;
  const [collapsed, setCollapsed] = createSignal<ReadonlySet<string>>(new Set());

  const groups = (): ProcessGroup[] => {
    const byProcess = new Map<string, ProcessGroup>();
    for (const entry of stats.entries()) {
      let group = byProcess.get(entry.process);
      if (!group) {
        group = {
          key: entry.process,
          label: entry.process === "" ? "Unknown process" : entry.process,
          entries: [],
          requests: 0,
          ok: 0,
          failed: 0,
        };
        byProcess.set(entry.process, group);
      }
      group.entries.push(entry);
      group.requests += entry.requests;
      group.ok += entry.ok;
      group.failed += entry.failed;
    }
    const groups = [...byProcess.values()];
    groups.sort((a, b) => b.requests - a.requests || a.label.localeCompare(b.label));
    for (const group of groups) {
      group.entries.sort((a, b) => b.requests - a.requests || a.host.localeCompare(b.host));
    }
    return groups;
  };

  const toggle = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

  const totalRequests = () => stats.entries().reduce((sum, entry) => sum + entry.requests, 0);
  const totalFailed = () => stats.entries().reduce((sum, entry) => sum + entry.failed, 0);

  return (
    <div
      classList={{
        "flex w-full flex-col": true,
        // mobile fills the window (the dock sits flush at its bottom);
        // desktop flows inside the scrollable content zone
        "min-h-[calc(100vh-3.5rem)]": isMobile(),
        "min-h-0": !isMobile(),
      }}
    >
      <div class="container mx-auto max-w-2xl space-y-4 p-4">
        <div
          classList={{
            "sticky z-40 -mx-4 rounded-2xl border border-base-content/10 bg-base-100/60 px-4 py-2.5 shadow-sm backdrop-blur-md": true,
            "top-14": isMobile(),
            "top-0": !isMobile(),
          }}
        >
          <div class="flex items-center gap-3">
            <A
              href="/"
              class="inline-flex h-8 shrink-0 cursor-pointer items-center justify-center gap-1 rounded-full bg-base-content/10 px-3 text-sm font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
            >
              <TbOutlineArrowLeft size={16} />
              Back
            </A>
            <div class="min-w-0">
              <h1 class="truncate text-lg font-bold">Session statistics</h1>
              <p class="truncate text-xs text-base-content/50">
                {groups().length} processes · {stats.entries().length} hosts · {totalRequests()}{" "}
                requests · {totalFailed()} failed
                <Show when={connected()}> · counting</Show>
              </p>
            </div>
          </div>
        </div>

        <Show
          when={stats.entries().length > 0}
          fallback={
            <Empty
              icon={TbOutlineWorld}
              title="No requests yet"
              description={
                connected()
                  ? "Open something through the proxy — every host shows up here under the process that asked for it."
                  : "Connect the proxy and open something — every host shows up here under the process that asked for it."
              }
            />
          }
        >
          <div class="space-y-3">
            <For each={groups()}>
              {(group) => (
                <ProcessGroupSection
                  group={group}
                  collapsed={collapsed().has(group.key)}
                  onToggle={() => toggle(group.key)}
                />
              )}
            </For>
          </div>
        </Show>
      </div>

      {/* the same dock geometry as Home: full window width (within the page
          padding), pinned to the window bottom — mt-auto holds it there
          while the summary above is shorter than the viewport */}
      <div class="mt-auto w-full px-4 pb-6 pt-4">
        <h2 class="px-1 pb-2 text-xs font-semibold uppercase tracking-widest text-base-content/50">
          Live traffic
        </h2>
        <div class="h-72">
          <TrafficDock />
        </div>
      </div>
    </div>
  );
};
