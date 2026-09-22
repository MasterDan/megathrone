import type { Component } from "solid-js";
import { For, Show, createSignal } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { Dynamic } from "solid-js/web";
import { TbOutlineAdjustments, TbOutlineRoute, TbOutlineSettings } from "solid-icons/tb";

import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { GeneralSection } from "@/components/settings/GeneralSection";
import { RoutingSection } from "@/components/settings/RoutingSection";

type TabId = "general" | "routing";

const TABS: Array<{ id: TabId; label: string; icon: Component<{ class?: string }> }> = [
  { id: "general", label: "General", icon: TbOutlineAdjustments },
  { id: "routing", label: "Routing", icon: TbOutlineRoute },
];

/**
 * Desktop settings: General and Routing as page-header tabs. DPI strategies
 * are a page of their own (/dpi); a legacy ?tab=dpi deep link redirects
 * there. Only the active tab is mounted (mirrors the mobile page).
 */
export const DesktopSettings: Component = () => {
  const location = useLocation();
  const navigate = useNavigate();
  const requested = new URLSearchParams(location.search).get("tab");

  if (requested === "dpi") {
    navigate("/dpi", { replace: true });
  }

  const [tab, setTab] = createSignal<TabId>(requested === "routing" ? "routing" : "general");

  return (
    <DesktopPage
      title={
        <h1 class="flex items-center gap-2 text-lg font-bold">
          <TbOutlineSettings size={20} class="text-base-content/60" />
          Settings
        </h1>
      }
      actions={
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
      }
    >
      <div class="mx-auto max-w-2xl">
        <Show when={tab() === "general"}>
          <GeneralSection />
        </Show>
        <Show when={tab() === "routing"}>
          <RoutingSection />
        </Show>
      </div>
    </DesktopPage>
  );
};
