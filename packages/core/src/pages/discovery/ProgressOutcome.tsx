import type { Component } from "solid-js";
import { Show } from "solid-js";
import { TbOutlineCircleCheck, TbOutlineCircleX } from "solid-icons/tb";

import type { DiscoverySourceProgress } from "@/hooks/data/useDiscovery";

/** The run outcome chip of one catalog row: queued, in flight, done
 *  (endpoint count) or failed (error text). */
export const ProgressOutcome: Component<{
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
