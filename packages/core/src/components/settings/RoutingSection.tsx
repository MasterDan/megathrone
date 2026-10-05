import type { Component } from "solid-js";
import { For, Show, createEffect, createSignal } from "solid-js";
import { useNavigate } from "@solidjs/router";
import {
  TbOutlineAlertTriangle,
  TbOutlinePlus,
  TbOutlineRoute,
  TbOutlineX,
} from "solid-icons/tb";

import { OptionTabs } from "@/components/common/daisy-ui/OptionTabs";
import { CategoryModal } from "@/components/settings/CategoryModal";
import { ACTION_ITEMS } from "@/components/settings/routingOptions";
import { useDpiSettings, useDpiStrategies } from "@/hooks/data/useDpi";
import { useRouting } from "@/hooks/data/useRouting";
import { useTestSites } from "@/hooks/data/useTestSites";
import type { RouteAction, TestSiteCategory } from "@/types";
import type { RoutingOption } from "@/components/settings/routingOptions";

/** Settings section: routing. One row per URL category with its outbound
 *  switch — DPI tunnel, proxy or direct; clicking the row opens the
 *  category's rule editor page. Below, the fallback action for hosts no
 *  category claims. While DPI is off or no strategy is selected, the DPI
 *  option disappears from the fallback (a stale dpi fallback is reset to
 *  proxy) and a warning says the DPI routes degrade to the fallback.
 *  Applies on the next connect. */
export const RoutingSection: Component = () => {
  const routing = useRouting();
  const sites = useTestSites();
  const dpiSettings = useDpiSettings();
  const strategies = useDpiStrategies();
  const navigate = useNavigate();

  const [newOpened, setNewOpened] = createSignal(false);

  const categories = () => sites.categories();
  const fallback = () => routing.config()?.fallback ?? "proxy";
  const loading = () => routing.loading() || sites.loading();
  const error = () => sites.error() ?? routing.error();
  const anyDpi = () =>
    fallback() === "dpi" || categories().some((category) => category.action === "dpi");

  // judge the DPI readiness only once both snapshots have loaded —
  // before that "unavailable" must not flash warnings or reset anything
  const dpiStateLoaded = () => !dpiSettings.loading() && !strategies.loading();
  const dpiEnabled = () => dpiSettings.settings()?.enabled ?? false;
  const hasActiveStrategy = () => strategies.strategies().some((strategy) => strategy.isActive);
  const dpiUnavailable = () => dpiStateLoaded() && (!dpiEnabled() || !hasActiveStrategy());

  const fallbackItems = (): Array<RoutingOption<RouteAction>> =>
    dpiUnavailable() ? ACTION_ITEMS.filter((item) => item.value !== "dpi") : ACTION_ITEMS;

  const dpiWarning = () => {
    if (!anyDpi() || !dpiUnavailable()) {
      return null;
    }
    return dpiEnabled()
      ? "No strategy selected — DPI routes will be sent to the fallback (Settings → DPI)."
      : "DPI is off — DPI routes will be sent to the fallback (Settings → DPI).";
  };

  // the DPI option vanished from the fallback (DPI off / strategy
  // deselected): a stale dpi selection degrades to proxy instead of
  // failing every connect
  createEffect(() => {
    if (dpiUnavailable() && routing.config()?.fallback === "dpi") {
      void routing.setFallback("proxy");
    }
  });

  const openCategory = (category: TestSiteCategory) => {
    navigate(`/settings/categories/${category.id}`);
  };

  return (
    <section class="card bg-base-content/5 shadow-sm">
      <div class="card-body gap-3 p-4 sm:p-5">
        <div class="flex items-start justify-between gap-2">
          <div>
            <h2 class="flex items-center gap-2 text-lg font-bold">
              <TbOutlineRoute size={20} class="text-base-content/50" />
              Routing
            </h2>
            <p class="mt-1 text-sm text-base-content/60">
              URL categories and where their rules go — DPI tunnel, proxy or direct. Everything
              unmatched goes to the fallback. Click a category to edit its rules. Applies on the
              next connect.
            </p>
          </div>
          <button
            type="button"
            class="inline-flex h-8 shrink-0 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-primary/20 px-3 text-sm font-medium whitespace-nowrap text-primary transition-colors hover:bg-primary/30 disabled:pointer-events-none disabled:opacity-40"
            disabled={sites.busy()}
            onClick={() => setNewOpened(true)}
          >
            <TbOutlinePlus size={16} />
            Category
          </button>
        </div>

        <Show
          when={!loading()}
          fallback={<span class="loading loading-spinner loading-sm" />}
        >
          <Show when={dpiWarning()}>
            <div class="alert alert-warning py-2 text-sm">
              <span>{dpiWarning()}</span>
            </div>
          </Show>

          <Show
            when={categories().length > 0}
            fallback={
              <div class="flex items-center gap-2 rounded-xl bg-base-200/60 px-3 py-4 text-sm text-base-content/50">
                <TbOutlineAlertTriangle size={16} class="shrink-0" />
                No categories yet — create one to give the tests something to probe.
              </div>
            }
          >
            <ul class="space-y-2">
              <For each={categories()}>
                {(category) => {
                  const openOnKey = (event: KeyboardEvent) => {
                    if (event.target !== event.currentTarget) {
                      return;
                    }
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      openCategory(category);
                    }
                  };
                  return (
                    <li
                      role="button"
                      tabIndex={0}
                      title="Edit rules"
                      class="flex cursor-pointer flex-wrap items-center justify-between gap-x-3 gap-y-1 rounded-xl bg-base-200/60 px-3 py-2 transition-colors hover:bg-base-200 focus:outline-none"
                      onClick={() => openCategory(category)}
                      onKeyDown={openOnKey}
                    >
                      <span class="min-w-0 px-1 py-0.5">
                        <span class="block truncate text-sm font-medium">{category.name}</span>
                        <span class="block text-xs text-base-content/50">
                          {category.rules.length} rules
                        </span>
                      </span>
                      {/* the action switcher must not open the category page */}
                      <span onClick={(event) => event.stopPropagation()}>
                        <OptionTabs
                          items={ACTION_ITEMS}
                          value={category.action}
                          onChange={(value) => void sites.setAction(category.id, value)}
                        />
                      </span>
                    </li>
                  );
                }}
              </For>
            </ul>
          </Show>

          <div class="border-t border-base-300/60 pt-3">
            <span class="label-text mb-1 block">Fallback</span>
            <OptionTabs
              items={fallbackItems()}
              value={fallback()}
              onChange={(value) => void routing.setFallback(value)}
            />
          </div>

          <Show when={error() && !newOpened()}>
            <div class="alert alert-error py-2 text-sm">
              <span class="break-all">{error()}</span>
              <TbOutlineX size={16} />
            </div>
          </Show>
        </Show>
      </div>

      <CategoryModal
        opened={newOpened}
        setOpened={setNewOpened}
        busy={sites.busy}
        error={sites.error}
        category={() => null}
        onSubmit={(name) => sites.addCategory(name)}
      />
    </section>
  );
};
