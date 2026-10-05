import type { Component } from "solid-js";
import { Match, Show, Switch } from "solid-js";
import { TbOutlineInfoCircle } from "solid-icons/tb";

import type { EndpointItem } from "@/types";
import { UrlScoreBadge, UrlTestButton } from "./UrlScoreBadge";

export const PROTOCOL_BADGES: Record<string, string> = {
  vless: "bg-sky-600 text-sky-50",
  vmess: "bg-cyan-600 text-cyan-50",
  trojan: "bg-purple-600 text-purple-50",
  shadowsocks: "bg-pink-600 text-pink-50",
  hysteria2: "bg-orange-600 text-orange-50",
  tuic: "bg-fuchsia-600 text-fuchsia-50",
};

interface Props {
  item: EndpointItem;
  selected: boolean;
  onToggle: () => void;
  onInfo: () => void;
  /** Opens the deep-probe details modal — handled above the grid, so the
   *  modal survives card re-renders while a scan rebuilds the list. */
  onUrlChecks: (item: EndpointItem, error?: string | null) => void;
}

/** Fixed card height so the endpoint grid can virtualize by row. The inline
 *  style below is the single source of truth — import this instead of a
 *  hardcoded number. */
export const ENDPOINT_CARD_HEIGHT = 120;

export const EndpointCard: Component<Props> = (props) => {
  const handleKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      props.onToggle();
    }
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-pressed={props.selected}
      class="card cursor-pointer bg-base-content/5 text-left shadow-sm transition-shadow hover:shadow-md focus:outline-none"
      style={{ height: `${ENDPOINT_CARD_HEIGHT}px` }}
      classList={{
        "bg-success/10": props.selected,
        "ring-1": props.selected,
        "ring-success/40": props.selected,
      }}
      onClick={props.onToggle}
      onKeyDown={handleKeyDown}
      title={props.item.tag}
    >
      <div class="card-body gap-1 p-3">
        <div class="flex items-center justify-between gap-2">
          <span class={`badge badge-sm shrink-0 ${PROTOCOL_BADGES[props.item.protocol] ?? "badge-ghost"}`}>
            {props.item.protocol}
          </span>
          <div class="flex min-w-0 items-center gap-1">
            <Show
              when={props.item.available !== null}
              fallback={<span class="text-[11px] text-base-content/30">not tested</span>}
            >
              <Show
                when={props.item.latencyMs != null}
                fallback={<span class="truncate text-[11px] font-medium text-error">unreachable</span>}
              >
                <span
                  class={`truncate text-[11px] font-medium ${props.item.available ? "text-success" : "text-error"}`}
                >
                  {props.item.latencyMs} ms
                </span>
              </Show>
            </Show>
            <Switch>
              <Match when={props.item.available === true && props.item.urlTotal > 0}>
                <UrlScoreBadge item={props.item} onShowChecks={props.onUrlChecks} />
              </Match>
              <Match when={props.item.urlTotal === 0}>
                <UrlTestButton item={props.item} onShowChecks={props.onUrlChecks} />
              </Match>
            </Switch>
            <button
              type="button"
              class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
              title="Endpoint details"
              aria-label={`Details for ${props.item.tag}`}
              onClick={(event) => {
                event.stopPropagation();
                props.onInfo();
              }}
            >
              <TbOutlineInfoCircle size={14} />
            </button>
          </div>
        </div>
        <p class="line-clamp-2 break-words text-sm font-medium leading-snug">{props.item.tag}</p>
        <p class="truncate text-[11px] text-base-content/50">
          {props.item.server}:{props.item.serverPort}
        </p>
      </div>
    </div>
  );
};
