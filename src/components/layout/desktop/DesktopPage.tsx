import type { Component, JSXElement } from "solid-js";
import { Show } from "solid-js";

import { ScrollZoneProvider } from "@/contexts/scrollZone";

interface DesktopPageProps {
  /** Page header above the content zone (title, page-level actions). */
  title?: JSXElement;
  /** Right side of the header (buttons, tabs). */
  actions?: JSXElement;
  /** Pinned to the bottom of the content zone, below the scrollable area —
   *  the home page's traffic charts live here. */
  footer?: JSXElement;
  children: JSXElement;
}

/**
 * One desktop page: an optional header row and the rounded content zone
 * with its own scrollbar. The zone exposes itself as the scroll zone
 * (contexts/scrollZone) so scroll-aware content (the virtualized endpoint
 * grid) follows it instead of the document window.
 */
export const DesktopPage: Component<DesktopPageProps> = (props) => {
  let zone: HTMLDivElement | undefined;

  return (
    <div class="flex h-full min-h-0 flex-col gap-3 p-4">
      <Show when={props.title}>
        <header class="flex shrink-0 items-center gap-3 px-1">
          <div class="min-w-0 flex-1">{props.title}</div>
          <Show when={props.actions}>
            <div class="flex shrink-0 items-center gap-2">{props.actions}</div>
          </Show>
        </header>
      </Show>
      <div class="flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl border border-base-content/10 bg-base-content/[0.02]">
        <div ref={zone} class="min-h-0 flex-1 overflow-y-auto p-4">
          <ScrollZoneProvider scroller={() => zone ?? null}>{props.children}</ScrollZoneProvider>
        </div>
        <Show when={props.footer}>
          <div class="shrink-0">{props.footer}</div>
        </Show>
      </div>
    </div>
  );
};
