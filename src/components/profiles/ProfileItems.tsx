import type { Component } from "solid-js";
import { Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import {
  VirtualWindowGrid,
  type GridBreakpoint,
  type VirtualWindowGridApi,
} from "@/components/common/VirtualWindowGrid";
import { useProfileItems } from "@/hooks/data/useProfileItems";
import { LATENCY_PROGRESS_EVENT, PROFILES_CHANGED_EVENT } from "@/types";
import type { EndpointItem, LatencyProgress, ProfilesChangedPayload } from "@/types";
import { ENDPOINT_CARD_HEIGHT, EndpointCard } from "./EndpointCard";
import { ItemDetailModal } from "./ItemDetailModal";
import { UrlChecksModal } from "./UrlChecksModal";

/** Imperative surface for the page: reveal an endpoint in the windowed
 *  grid (resolve its canonical index, then scroll there). */
export interface ProfileItemsApi {
  revealItem: (itemId: number) => void;
}

interface Props {
  profileId: number;
  /** Endpoint count of the profile — sizes the scrollable area, so the
   *  scrollbar covers the whole list before any of it is fetched. */
  total: number;
  /** The currently selected endpoint — kept in the list (highlighted with
   *  the success border; it also lives in the compact selection row above). */
  selectedId: number | null;
  /** Card click / Enter — picks the endpoint (switches to manual). */
  onSelect: (item: EndpointItem) => void;
  /** Receives the imperative API (reveal-an-endpoint) once, on mount. */
  exposeApi?: (api: ProfileItemsApi) => void;
}

/** Column ladder of the endpoints grid, mirroring the plain Tailwind classes
 *  it replaced: 1 / sm:2 / lg:3 / 2xl:4. */
const GRID_BREAKPOINTS: GridBreakpoint[] = [
  { minWidth: 640, columns: 2 },
  { minWidth: 1024, columns: 3 },
  { minWidth: 1536, columns: 4 },
];
const GRID_GAP = 8;

/** Room the sticky page header takes in the viewport when stuck — the
 *  reveal scroll lands the target card just below it. */
const REVEAL_TOP_OFFSET = 160;

/** One grid cell: a not-yet-fetched slot or a loaded endpoint. The endpoint
 *  order is the server's canonical one (see `profile_items`). */
type Cell = { kind: "skeleton" } | { kind: "endpoint"; item: EndpointItem };

export const ProfileItems: Component<Props> = (props) => {
  const {
    slots,
    error,
    ensureRange,
    refreshLoaded,
    invalidate,
    applyLatency,
    reset: reloadItems,
  } = useProfileItems(() => props.profileId, () => props.total);
  const [detailItem, setDetailItem] = createSignal<EndpointItem | null>(null);

  /** The open deep-probe details modal lives here, above the grid: a card
   *  re-render (scan reordering the list, latency batches replacing slot
   *  objects) would unmount a modal owned by the card and slam it shut. */
  const [urlChecks, setUrlChecks] = createSignal<{ item: EndpointItem; error: string | null } | null>(
    null,
  );

  /** Imperative API of the grid — handed over by the grid on mount. */
  let gridApi: VirtualWindowGridApi | undefined;

  /** The server's canonical index is the only trustworthy position (loaded
   *  slots can be stale after an order change), so it is always resolved
   *  before scrolling. */
  const revealItem = (itemId: number) => {
    void invoke<number | null>("profile_item_index", {
      profileId: props.profileId,
      itemId,
    }).then(
      (index) => {
        if (index != null) {
          gridApi?.scrollToIndex(index, REVEAL_TOP_OFFSET);
        }
      },
      () => {
        // best-effort reveal: a failed index lookup just does nothing
      },
    );
  };

  /** Visible cell range as last reported by the grid (includes its overscan
   *  rows) — drives the "is the user watching the top of the list" check. */
  let visibleRange: [number, number] = [0, 0];

  // A newly-available endpoint reorders the list from the top, so while the
  // user watches the top region the visible window re-reads in the fresh
  // order. Bursts coalesce (trailing debounce) but a steady trickle of new
  // successes must not starve the update (max wait). Deep in the list the
  // reorder is just churn — there it waits for the scan-end invalidation.
  const ORDER_REFRESH_DEBOUNCE_MS = 1000;
  const ORDER_REFRESH_MAX_WAIT_MS = 2500;
  let orderRefreshTimer: number | undefined;
  let orderRefreshFirstAt = 0;

  const cancelOrderRefresh = () => {
    if (orderRefreshTimer !== undefined) {
      window.clearTimeout(orderRefreshTimer);
    }
    orderRefreshTimer = undefined;
    orderRefreshFirstAt = 0;
  };

  const fireOrderRefresh = () => {
    orderRefreshTimer = undefined;
    orderRefreshFirstAt = 0;
    // "top, or almost top": the window starts within one viewport-span of
    // the list head (the span already carries the grid's overscan margin)
    const [start, end] = visibleRange;
    if (start <= end - start) {
      refreshLoaded();
    }
  };

  const scheduleOrderRefresh = () => {
    if (!orderRefreshFirstAt) {
      orderRefreshFirstAt = Date.now();
    }
    const delay = Math.min(
      ORDER_REFRESH_DEBOUNCE_MS,
      Math.max(0, orderRefreshFirstAt + ORDER_REFRESH_MAX_WAIT_MS - Date.now()),
    );
    if (orderRefreshTimer !== undefined) {
      window.clearTimeout(orderRefreshTimer);
    }
    orderRefreshTimer = window.setTimeout(fireOrderRefresh, delay);
  };

  const cells = createMemo<Cell[]>(() =>
    slots().map((item): Cell => (item ? { kind: "endpoint", item } : { kind: "skeleton" })),
  );

  const renderCell = (cell: Cell) => {
    switch (cell.kind) {
      case "skeleton":
        return (
          <div
            class="card bg-base-content/5"
            style={{ height: `${ENDPOINT_CARD_HEIGHT}px` }}
          >
            <div class="card-body gap-1 p-3">
              <span class="skeleton h-5 w-16 rounded-full" />
              <span class="skeleton h-4 w-3/4" />
              <span class="skeleton h-3.5 w-1/2" />
            </div>
          </div>
        );
      case "endpoint":
        return (
          <EndpointCard
            item={cell.item}
            selected={props.selectedId === cell.item.id}
            onToggle={() => props.onSelect(cell.item)}
            onInfo={() => setDetailItem(cell.item)}
            onUrlChecks={(item, error) => setUrlChecks({ item, error: error ?? null })}
          />
        );
    }
  };

  const unlisteners: Array<() => void> = [];
  let disposed = false;
  const keep = (promise: Promise<() => void>) => {
    void promise.then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    });
  };

  onMount(() => {
    props.exposeApi?.({ revealItem });
    keep(
      listen<ProfilesChangedPayload>(PROFILES_CHANGED_EVENT, (event) => {
        const payload = event.payload;
        if (payload.profileId !== props.profileId) {
          return;
        }
        if (payload.kind === "content") {
          // the list may shrink a lot after an update; avoid showing empty space
          window.scrollTo({ top: 0 });
          reloadItems();
          cancelOrderRefresh();
          // the selection itself is re-resolved by useProfileSelection (raw-link key)
        } else if (payload.kind === "latency") {
          // the scan (or a point test) finished — the whole order may have
          // changed, not just the visible window
          cancelOrderRefresh();
          invalidate();
        }
      }),
    );

    keep(
      listen<LatencyProgress>(LATENCY_PROGRESS_EVENT, (event) => {
        if (event.payload.profileId !== props.profileId) {
          return;
        }
        const results = event.payload.results;
        // statuses of already-loaded cards update in place; a *newly* found
        // available endpoint (no loaded slot yet, or the slot wasn't
        // available) is the reorder trigger
        const before = slots();
        applyLatency(results);
        const newlyAvailable = results.some((result) => {
          if (!result.available) {
            return false;
          }
          const slot = before.find((item) => item?.id === result.id);
          return !slot || slot.available !== true;
        });
        if (newlyAvailable) {
          scheduleOrderRefresh();
        }
      }),
    );
  });
  onCleanup(() => {
    disposed = true;
    cancelOrderRefresh();
    unlisteners.forEach((unlisten) => unlisten());
  });

  const handleRangeChange = (start: number, end: number) => {
    visibleRange = [start, end];
    ensureRange(start, end);
  };

  return (
    <div class="space-y-3">
      <VirtualWindowGrid
        items={cells()}
        itemHeight={ENDPOINT_CARD_HEIGHT}
        gap={GRID_GAP}
        overscanRows={2}
        breakpoints={GRID_BREAKPOINTS}
        onRangeChange={handleRangeChange}
        onApi={(api) => (gridApi = api)}
      >
        {renderCell}
      </VirtualWindowGrid>

      <Show when={error()}>
        <div class="alert alert-error py-2 text-sm">
          <span class="break-all">{error()}</span>
        </div>
      </Show>

      <Show when={props.total > 0}>
        <p class="text-center text-xs text-base-content/40">end of list</p>
      </Show>

      <ItemDetailModal
        item={detailItem()}
        opened={() => detailItem() !== null}
        setOpened={(open) => {
          if (!open) {
            setDetailItem(null);
          }
        }}
      />

      <UrlChecksModal
        item={urlChecks()?.item ?? null}
        error={urlChecks()?.error ?? null}
        opened={() => urlChecks() !== null}
        setOpened={(open) => {
          if (!open) {
            setUrlChecks(null);
          }
        }}
      />
    </div>
  );
};
