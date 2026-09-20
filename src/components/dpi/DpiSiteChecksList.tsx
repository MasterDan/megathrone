import type { Component } from "solid-js";
import { For, Show, createMemo, createResource } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { TbOutlineCircleCheck, TbOutlineX } from "solid-icons/tb";

import type { DpiSiteStatus } from "@/types";

interface Props {
  strategyId: number;
}

/** Per-strategy site check details, grouped by category; fetched on mount
 *  so the popover and modal stay lazy (nothing is loaded until the user
 *  asks). */
export const DpiSiteChecksList: Component<Props> = (props) => {
  const [checks] = createResource(
    () => props.strategyId,
    (strategyId) => invoke<DpiSiteStatus[]>("dpi_strategy_urls", { strategyId }),
  );

  const grouped = createMemo(() => {
    const groups: { id: number; name: string; sites: DpiSiteStatus[] }[] = [];
    for (const status of checks() ?? []) {
      let group = groups.find((candidate) => candidate.id === status.categoryId);
      if (!group) {
        group = { id: status.categoryId, name: status.category, sites: [] };
        groups.push(group);
      }
      group.sites.push(status);
    }
    return groups;
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
        <For each={grouped()}>
          {(group) => (
            <div class="mb-3 last:mb-0">
              <p class="mb-1 text-[11px] font-semibold tracking-wide text-base-content/50 uppercase">
                {group.name}
              </p>
              <ul class="space-y-1">
                <For each={group.sites}>
                  {(status) => (
                    <li class="flex items-center justify-between gap-2">
                      <span class="min-w-0 truncate text-xs" title={status.url}>
                        {status.url}
                      </span>
                      <Show
                        when={status.ok === null}
                        fallback={
                          <Show
                            when={status.ok}
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
                            <span
                              class="flex shrink-0 items-center gap-0.5 text-xs font-medium text-success"
                              title="reachable"
                            >
                              <TbOutlineCircleCheck size={13} />
                              ok
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
      </Show>
    </Show>
  );
};
