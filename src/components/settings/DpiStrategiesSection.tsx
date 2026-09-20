import type { Component } from "solid-js";
import type { Accessor } from "solid-js";
import {
  For,
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
} from "solid-js";
import {
  TbOutlineActivity,
  TbOutlineAlertTriangle,
  TbOutlineCircleCheck,
  TbOutlinePlayerStop,
  TbOutlinePlus,
  TbOutlineX,
} from "solid-icons/tb";

import { Button } from "@/components/common/daisy-ui/Button";
import { TransitionCollapse } from "@/components/common/transitions/TransitionCollapse";
import { StrategyModal } from "@/components/dpi/StrategyModal";
import { StrategyScoreBadge } from "@/components/dpi/StrategyScoreBadge";
import { useDpiStrategies } from "@/hooks/data/useDpi";
import { useDpiTest } from "@/hooks/data/useDpiTest";
import type { DpiStrategy } from "@/types";

/** Best last-test score first; never-tested (or nothing to test against)
 *  strategies keep their stored order below. Stable sort, so the list
 *  reshuffles live as results arrive. */
function compareStrategies(a: DpiStrategy, b: DpiStrategy) {
  const score = (strategy: DpiStrategy) =>
    strategy.tested > 0 && strategy.urlTotal > 0 ? strategy.urlOk / strategy.urlTotal : -1;
  return score(b) - score(a);
}

interface Props {
  /** the DPI master toggle state (owned by DpiSection) */
  enabled: Accessor<boolean>;
  /** deep link (?strategy=<id>): scroll this strategy into view */
  revealStrategyId?: number | null;
  /** called once the reveal scroll happened (the caller cleans the URL) */
  onRevealDone?: () => void;
}

/**
 * The byedpi strategy list (Settings → DPI), shown only while DPI is on.
 * A strategy is the raw argument line passed to `ciadpi`; the marked one
 * runs together with the proxy whenever routing sends anything into the
 * DPI tunnel. The list is sorted by the last test results.
 */
export const DpiStrategiesSection: Component<Props> = (props) => {
  const { strategies, loading, busy, error, add, update, remove, select, applyTestResult } =
    useDpiStrategies();

  const [testErrors, setTestErrors] = createSignal<Record<number, string>>({});
  const { testing, stopping, progress, error: testError, test, cancel } = useDpiTest((result) => {
    applyTestResult(result);
    setTestErrors((previous) => {
      const next = { ...previous };
      if (result.error) {
        next[result.strategyId] = result.error;
      } else {
        delete next[result.strategyId];
      }
      return next;
    });
  });

  const [modalOpened, setModalOpened] = createSignal(false);
  const [editing, setEditing] = createSignal<DpiStrategy | null>(null);

  const sortedStrategies = createMemo(() => [...strategies()].sort(compareStrategies));

  // Deep link: scroll to the requested strategy once the list has rendered.
  // The wait also lets the section's collapse transition finish (it only
  // exists while DPI is on), so the positions are final before scrolling.
  createEffect(() => {
    const id = props.revealStrategyId;
    if (id == null || loading() || strategies().length === 0) {
      return;
    }
    const timer = window.setTimeout(() => {
      document
        .getElementById(`dpi-strategy-${id}`)
        ?.scrollIntoView({ block: "center", behavior: "smooth" });
      props.onRevealDone?.();
    }, 350);
    onCleanup(() => window.clearTimeout(timer));
  });

  const openAdd = () => {
    setEditing(null);
    setModalOpened(true);
  };

  const openEdit = (strategy: DpiStrategy) => {
    setEditing(strategy);
    setModalOpened(true);
  };

  const progressPercent = () => {
    const current = progress();
    return current && current.total > 0 ? (current.done / current.total) * 100 : 0;
  };

  return (
    <TransitionCollapse>
      <Show when={props.enabled()}>
        {/* spacing inside the animated block — a sibling margin would snap
            on unmount (see TransitionCollapse) */}
        <div class="pt-4">
          <section class="card bg-base-content/5 shadow-sm">
            <div class="card-body gap-3 p-4 sm:p-5">
              <div class="flex items-center justify-between gap-2">
                <h2 class="text-lg font-bold">Strategies</h2>
                <div class="flex items-center gap-1.5">
                  <Button
                    size="sm"
                    variant={testing() ? "error" : "primary"}
                    disabled={stopping()}
                    title={
                      testing()
                        ? "Stop the running test"
                        : "Run every strategy against the DPI-routed categories (Settings → Routing)"
                    }
                    onClick={() => (testing() ? void cancel() : void test())}
                  >
                    <Switch>
                      <Match when={stopping()}>
                        <span class="loading loading-spinner loading-xs" />
                        Stopping…
                      </Match>
                      <Match when={testing()}>
                        <TbOutlinePlayerStop size={16} />
                        Stop
                      </Match>
                      <Match when={true}>
                        <TbOutlineActivity size={16} />
                        Test
                      </Match>
                    </Switch>
                  </Button>
                  <Button size="sm" outline disabled={busy()} onClick={openAdd}>
                    <TbOutlinePlus size={16} />
                    Add
                  </Button>
                </div>
              </div>

              <p class="text-sm text-base-content/60">
                Each strategy is a raw <code>ciadpi</code> argument line — e.g.{" "}
                <code class="whitespace-nowrap">-s2 -d2</code>. The one marked{" "}
                <span class="badge badge-success badge-sm badge-outline">in use</span> runs
                together with the proxy whenever routing sends anything into the DPI tunnel.
              </p>

              <Show when={testing()}>
                <div>
                  <div class="mb-1 flex items-center justify-between text-xs text-base-content/60">
                    <span>Testing strategies…</span>
                    <span class="tabular-nums">
                      {progress()?.done ?? 0}/{progress()?.total ?? 0}
                    </span>
                  </div>
                  <progress
                    class="progress progress-primary h-2 w-full"
                    value={progressPercent()}
                    max={100}
                  />
                </div>
              </Show>

              <Show
                when={!loading() && strategies().length > 0}
                fallback={
                  <Show when={!loading()} fallback={<span class="loading loading-spinner" />}>
                    <p class="text-sm text-base-content/50">
                      No strategies yet — add one. A strategy with no arguments runs ciadpi with
                      its defaults.
                    </p>
                  </Show>
                }
              >
                <ul class="space-y-2">
                  <For each={sortedStrategies()}>
                    {(strategy) => {
                      const openOnKey = (event: KeyboardEvent) => {
                        if (event.target !== event.currentTarget) {
                          return;
                        }
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          openEdit(strategy);
                        }
                      };
                      return (
                        <li
                          id={`dpi-strategy-${strategy.id}`}
                          role="button"
                          tabIndex={0}
                          title="Edit strategy"
                          class="flex cursor-pointer items-center gap-2 rounded-xl bg-base-200/60 px-2 py-2 transition-colors hover:bg-base-200 focus:outline-none"
                          onClick={() => openEdit(strategy)}
                          onKeyDown={openOnKey}
                        >
                          <button
                            type="button"
                            class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
                            classList={{ "text-success": strategy.isActive }}
                            title={strategy.isActive ? "In use" : "Use this strategy"}
                            aria-label={
                              strategy.isActive ? "Active strategy" : "Activate strategy"
                            }
                            disabled={busy() || strategy.isActive}
                            onClick={(event) => {
                              event.stopPropagation();
                              void select(strategy.id);
                            }}
                          >
                            <TbOutlineCircleCheck size={18} />
                          </button>
                          <span class="min-w-0 flex-1">
                            <span class="flex flex-wrap items-center gap-x-2">
                              <span class="truncate text-sm font-medium">{strategy.name}</span>
                              <Show when={strategy.isActive}>
                                <span class="badge badge-success badge-sm shrink-0 badge-outline">
                                  in use
                                </span>
                              </Show>
                            </span>
                            <Show
                              when={strategy.args}
                              fallback={
                                <span class="block text-xs text-base-content/40">
                                  no arguments (defaults)
                                </span>
                              }
                            >
                              <code
                                class="block truncate font-mono text-xs text-base-content/60"
                                title={strategy.args}
                              >
                                {strategy.args}
                              </code>
                            </Show>
                          </span>
                          <Show when={testErrors()[strategy.id]} keyed>
                            {(message) => (
                              <span
                                class="shrink-0 text-warning"
                                title={`The test could not run this strategy: ${message}`}
                              >
                                <TbOutlineAlertTriangle size={16} />
                              </span>
                            )}
                          </Show>
                          <Show when={strategy.tested > 0 && strategy.urlTotal > 0}>
                            <StrategyScoreBadge strategy={strategy} />
                          </Show>
                        </li>
                      );
                    }}
                  </For>
                </ul>
              </Show>

              <Show when={(error() || testError()) && !modalOpened()}>
                <div class="alert alert-error py-2 text-sm">
                  <span class="break-all">{error() ?? testError()}</span>
                  <TbOutlineX size={16} />
                </div>
              </Show>
            </div>
          </section>

          <StrategyModal
            opened={modalOpened}
            setOpened={setModalOpened}
            busy={busy}
            error={error}
            strategy={editing}
            onAdd={add}
            onUpdate={update}
            onDelete={remove}
          />
        </div>
      </Show>
    </TransitionCollapse>
  );
};
