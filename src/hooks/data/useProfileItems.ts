import { createEffect, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

import type { EndpointItem, EndpointLatency } from "@/types";

/** Windowed random-access cache: the grid is sized by the profile's total
 *  endpoint count and the visible slice is fetched on demand, so the
 *  scrollbar covers the whole list from the start.
 *
 *  Requests are aligned to multiples of FETCH_CHUNK: crossing a chunk
 *  boundary while scrolling costs exactly one request, and staying inside
 *  one costs none. The chunk also comfortably covers any viewport window
 *  plus overscan in one go. */
const FETCH_CHUNK = 120;

export function useProfileItems(profileId: () => number, total: () => number) {
  /** Slot per endpoint, in the server's canonical order; undefined = not
   *  (yet) fetched — the grid shows a skeleton there. */
  const [slots, setSlots] = createSignal<(EndpointItem | undefined)[]>([]);
  const [error, setError] = createSignal<string | null>(null);

  // Fetch bookkeeping — non-reactive on purpose, it only steers requests.
  /** Bumped on every reset; responses from an older epoch are dropped. */
  let epoch = 0;
  /** Merged, chunk-aligned, non-overlapping spans already requested. */
  let requested: Array<[number, number]> = [];
  /** The last window ensureRange() was asked for — what refreshLoaded() re-reads. */
  let lastWindow: [number, number] = [0, 0];

  /** Spans of [start, end) not covered by `requested` yet. */
  const missingSpans = (start: number, end: number): Array<[number, number]> => {
    const result: Array<[number, number]> = [];
    let cursor = start;
    for (const [from, to] of requested) {
      if (to <= cursor) {
        continue;
      }
      if (from >= end) {
        break;
      }
      if (from > cursor) {
        result.push([cursor, Math.min(from, end)]);
      }
      cursor = Math.max(cursor, to);
      if (cursor >= end) {
        return result;
      }
    }
    if (cursor < end) {
      result.push([cursor, end]);
    }
    return result;
  };

  const markRequested = (start: number, end: number) => {
    requested.push([start, end]);
    requested.sort((a, b) => a[0] - b[0]);
    const merged: Array<[number, number]> = [];
    for (const span of requested) {
      const last = merged[merged.length - 1];
      if (last && span[0] <= last[1]) {
        last[1] = Math.max(last[1], span[1]);
      } else {
        merged.push([span[0], span[1]]);
      }
    }
    requested = merged;
  };

  const unmarkRange = (start: number, end: number) => {
    requested = requested.flatMap(([from, to]): Array<[number, number]> => {
      if (to <= start || from >= end) {
        return [[from, to]];
      }
      const parts: Array<[number, number]> = [];
      if (from < start) {
        parts.push([from, start]);
      }
      if (to > end) {
        parts.push([end, to]);
      }
      return parts;
    });
  };

  const fetchSpan = async (start: number, end: number, validEpoch: number) => {
    try {
      const page = await invoke<EndpointItem[]>("profile_items", {
        profileId: profileId(),
        offset: start,
        limit: end - start,
      });
      if (validEpoch !== epoch) {
        return; // a reset happened while the request was in flight
      }
      setSlots((previous) => {
        const next = previous.slice();
        for (let i = 0; i < page.length && start + i < next.length; i += 1) {
          next[start + i] = page[i];
        }
        return next;
      });
    } catch (cause) {
      if (validEpoch === epoch) {
        // let the next window change retry this span
        unmarkRange(start, end);
        setError(String(cause));
      }
    }
  };

  /** Make sure every slot of the half-open [start, end) is loaded (or its
   *  request is in flight). Indices are in the server's canonical order. */
  const ensureRange = (start: number, end: number) => {
    const bound = slots().length;
    const from = Math.max(0, start);
    const to = Math.min(bound, end);
    if (to <= from) {
      return;
    }
    lastWindow = [from, to];
    for (const [spanStart, spanEnd] of missingSpans(from, to)) {
      // align to chunk boundaries so scrolling inside a chunk is free
      const chunkStart = Math.floor(spanStart / FETCH_CHUNK) * FETCH_CHUNK;
      const chunkEnd = Math.min(bound, Math.ceil(spanEnd / FETCH_CHUNK) * FETCH_CHUNK);
      if (chunkEnd <= chunkStart) {
        continue;
      }
      markRequested(chunkStart, chunkEnd);
      void fetchSpan(chunkStart, chunkEnd, epoch);
    }
  };

  /** Re-read the currently visible window (e.g. the live top-of-list
   *  reorder while a scan is running). */
  const refreshLoaded = () => {
    const [from, to] = lastWindow;
    if (to <= from) {
      return;
    }
    unmarkRange(from, to);
    ensureRange(from, to);
  };

  /** Drop all fetch bookkeeping after a wholesale order change (the scan
   *  finished, a point test ran): the whole canonical order is different
   *  now, so every cached window is potentially stale. The visible window
   *  re-reads immediately; the rest re-reads lazily when scrolled to (old
   *  slot values stay until replaced — no skeleton flash). */
  const invalidate = () => {
    epoch += 1;
    requested = [];
    const [from, to] = lastWindow;
    if (to > from) {
      ensureRange(from, to);
    }
  };

  /** Patch loaded slots in place with live probe results; slots on
   *  unfetched windows come fresh from the DB when scrolled to. */
  const applyLatency = (results: EndpointLatency[]) => {
    if (results.length === 0) {
      return;
    }
    const byId = new Map(results.map((result) => [result.id, result]));
    setSlots((previous) =>
      previous.map((item) => {
        if (!item) {
          return item;
        }
        const result = byId.get(item.id);
        return result
          ? {
              ...item,
              available: result.available,
              latencyMs: result.latencyMs,
              urlOk: result.urlOk,
              urlTotal: result.urlTotal,
            }
          : item;
      }),
    );
  };

  const reset = () => {
    epoch += 1;
    requested = [];
    lastWindow = [0, 0];
    setError(null);
    setSlots(new Array(total()).fill(undefined));
  };

  // (Re)build the slot array on mount and whenever the profile or its
  // endpoint count changes; a same-count content refresh goes through
  // `reset()` (event-driven).
  createEffect(() => {
    void profileId();
    void total();
    reset();
  });

  return {
    slots,
    error,
    ensureRange,
    refreshLoaded,
    invalidate,
    applyLatency,
    reset,
  };
}
