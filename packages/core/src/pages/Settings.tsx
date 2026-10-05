import type { Component } from "solid-js";
import { For, Show, createSignal } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { Dynamic } from "solid-js/web";
import {
  TbOutlineAdjustments,
  TbOutlineRoute,
  TbOutlineSettings,
  TbOutlineShieldLock,
} from "solid-icons/tb";

import { DpiSection } from "@/components/settings/DpiSection";
import { GeneralSection } from "@/components/settings/GeneralSection";
import { RoutingSection } from "@/components/settings/RoutingSection";

type TabId = "dpi" | "routing" | "general";

const TABS: Array<{ id: TabId; label: string; icon: Component<{ class?: string }> }> = [
  { id: "general", label: "General", icon: TbOutlineAdjustments },
  { id: "dpi", label: "DPI", icon: TbOutlineShieldLock },
  { id: "routing", label: "Routing", icon: TbOutlineRoute },
];

/**
 * Settings grouped into dock-style tabs. Only the active tab is mounted —
 * among other things this keeps cross-tab state changes (e.g. category
 * action flips reshaping the Routing list) from visibly re-shuffling a
 * section the user is not looking at.
 */
export const Settings: Component = () => {
  const location = useLocation();
  const navigate = useNavigate();
  // deep links (e.g. back from a category page) pick the starting tab
  const search = new URLSearchParams(location.search);
  const requested = search.get("tab");
  const initial: TabId = TABS.some((entry) => entry.id === requested)
    ? (requested as TabId)
    : "general";
  const [tab, setTab] = createSignal<TabId>(initial);

  // deep link from Home (?strategy=<id>): the DPI tab scrolls to that
  // strategy once its list has rendered, then the param is cleaned up
  const requestedStrategy = Number(search.get("strategy"));
  const [revealStrategy, setRevealStrategy] = createSignal<number | null>(
    Number.isInteger(requestedStrategy) ? requestedStrategy : null,
  );

  return (
    <div class="container mx-auto max-w-2xl p-4">
      <div class="sticky top-14 z-40 -mx-4 mb-4 flex flex-wrap items-center justify-between gap-x-2 gap-y-1 bg-base-100/60 px-4 py-1.5 backdrop-blur-md">
        <h1 class="flex items-center gap-2 text-xl font-bold">
          <TbOutlineSettings size={22} class="text-base-content/60" />
          Settings
        </h1>

        <nav aria-label="Settings sections">
          <div class="flex items-center gap-1 rounded-2xl border border-base-content/10 bg-base-100/60 p-1 shadow-lg">
            <For each={TABS}>
              {(entry) => (
                <button
                  type="button"
                  role="tab"
                  aria-selected={tab() === entry.id}
                  class="btn btn-ghost btn-sm gap-2 rounded-xl"
                  classList={{
                    "text-base-content/60": tab() !== entry.id,
                    "bg-base-content/10 text-base-content": tab() === entry.id,
                  }}
                  onClick={() => setTab(entry.id)}
                >
                  <Dynamic component={entry.icon} class="size-4" />
                  {entry.label}
                </button>
              )}
            </For>
          </div>
        </nav>
      </div>

      <Show when={tab() === "dpi"}>
        <DpiSection
          revealStrategyId={revealStrategy()}
          onRevealDone={() => {
            setRevealStrategy(null);
            navigate("/settings?tab=dpi", { replace: true });
          }}
        />
      </Show>
      <Show when={tab() === "routing"}>
        <RoutingSection />
      </Show>
      <Show when={tab() === "general"}>
        <GeneralSection />
      </Show>
    </div>
  );
};
