import type { Component } from "solid-js";
import { For, Show, createMemo, createResource, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { TbOutlineCircleCheck, TbOutlineX } from "solid-icons/tb";

import { LATENCY_PROGRESS_EVENT } from "@/types";
import type { EndpointUrlStatus, LatencyProgress } from "@/types";

interface Props {
  endpointId: number;
  /** Bumped by the modal after a successful retest — a source change makes
   *  the resource re-read the (replaced) rows. */
  refreshToken?: number;
}

/** Per-endpoint deep-probe details (proxy-routed category URLs), grouped by
 *  category; fetched on mount so the modal stays lazy (nothing is loaded
 *  until the user asks). A running scan keeps the open list fresh: batches
 *  touching this endpoint re-read it. */
export const UrlChecksList: Component<Props> = (props) => {
  const [checks, { refetch }] = createResource(
    () => [props.endpointId, props.refreshToken ?? 0] as const,
    ([endpointId]) => invoke<EndpointUrlStatus[]>("profile_endpoint_urls", { itemId: endpointId }),
  );

  onMount(() => {
    let disposed = false;
    void listen<LatencyProgress>(LATENCY_PROGRESS_EVENT, (event) => {
      if (event.payload.results.some((result) => result.id === props.endpointId)) {
        void refetch();
      }
    }).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        onCleanup(() => unlisten());
      }
    });
    onCleanup(() => {
      disposed = true;
    });
  });

  // the backend orders by category position; regroup while preserving it
  const groups = createMemo(() => {
    const byCategory = new Map<string, EndpointUrlStatus[]>();
    for (const check of checks() ?? []) {
      const list = byCategory.get(check.category) ?? [];
      list.push(check);
      byCategory.set(check.category, list);
    }
    return [...byCategory.entries()];
  });

  return (
    <Show
      when={!checks.loading}
      fallback={
        <div class="flex justify-center py-4">
          <span class="loading loading-spinner loading-sm" />
        </div>
      }
    >
      <Show
        when={!checks.error}
        fallback={<div class="alert alert-error py-2 text-xs">{String(checks.error)}</div>}
      >
        <div class="space-y-3">
          <For each={groups()}>
            {([category, entries]) => (
              <div>
                <p class="mb-1 text-[11px] font-semibold tracking-wide text-base-content/50 uppercase">
                  {category}
                </p>
                <ul class="space-y-1.5">
                  <For each={entries}>
                    {(check) => (
                      <li class="flex items-center justify-between gap-2">
                        <span class="min-w-0 truncate text-xs" title={check.url}>
                          {check.url}
                        </span>
                        <Show
                          when={check.available === null}
                          fallback={
                            <Show
                              when={check.available}
                              fallback={
                                <span
                                  class="flex shrink-0 items-center gap-0.5 text-xs font-medium text-error"
                                  title="unreachable"
                                >
                                  <TbOutlineX size={13} />
                                  fail
                                </span>
                              }
                            >
                              <span class="flex shrink-0 items-center gap-0.5 text-xs font-medium text-success">
                                <TbOutlineCircleCheck size={13} />
                                {check.latencyMs} ms
                              </span>
                            </Show>
                          }
                        >
                          <span class="shrink-0 text-xs text-base-content/40">not tested</span>
                        </Show>
                      </li>
                    )}
                  </For>
                </ul>
              </div>
            )}
          </For>
        </div>
      </Show>
    </Show>
  );
};
