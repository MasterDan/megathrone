import type { Accessor, ParentComponent } from "solid-js";
import { createContext, createSignal, onCleanup, onMount, useContext } from "solid-js";
import { listen } from "@tauri-apps/api/event";

import { TRAFFIC_STATS_EVENT } from "@/types";
import type { TrafficStats } from "@/types";

/** How many rate samples the charts keep — one per backend tick (~1s). */
export const TRAFFIC_WINDOW = 90;

export type TrafficKey = keyof TrafficStats;
export type TrafficRates = TrafficStats;
export type TrafficSeries = Record<TrafficKey, number[]>;

const KEYS: readonly TrafficKey[] = [
  "proxyUp",
  "proxyDown",
  "dpiUp",
  "dpiDown",
  "directUp",
  "directDown",
];

const zeroRates = (): TrafficRates => ({
  proxyUp: 0,
  proxyDown: 0,
  dpiUp: 0,
  dpiDown: 0,
  directUp: 0,
  directDown: 0,
});

const emptySeries = (): TrafficSeries => ({
  proxyUp: [],
  proxyDown: [],
  dpiUp: [],
  dpiDown: [],
  directUp: [],
  directDown: [],
});

/** The window starts fully zero-padded — the charts draw flat curves from
 *  the very first frame and fill in as samples arrive. */
const zeroSeries = (): TrafficSeries => {
  const series = emptySeries();
  for (const key of KEYS) {
    series[key] = new Array<number>(TRAFFIC_WINDOW).fill(0);
  }
  return series;
};

interface TrafficStatsStore {
  rates: Accessor<TrafficRates>;
  series: Accessor<TrafficSeries>;
}

const TrafficStatsContext = createContext<TrafficStatsStore>();

/**
 * Live session traffic: the backend polls the running sing-box's clash API
 * and emits cumulative per-outbound counters (`traffic-stats`, ~1 Hz); the
 * provider turns consecutive samples into per-second rates and keeps a
 * rolling window for the charts. Mounted once in `AppLayout`, above the
 * routes — the window keeps filling regardless of navigation, so the charts
 * survive a trip to another page and back. A cumulative counter going
 * backwards means a fresh session's poller took over — the window resets so
 * the new session starts from a clean, zero-padded chart.
 */
export const TrafficStatsProvider: ParentComponent = (props) => {
  const [rates, setRates] = createSignal<TrafficRates>(zeroRates());
  const [series, setSeries] = createSignal<TrafficSeries>(zeroSeries());

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let previous: { stats: TrafficStats; at: number } | null = null;

    void listen<TrafficStats>(TRAFFIC_STATS_EVENT, (event) => {
      if (disposed) {
        return;
      }
      const stats = event.payload;
      const at = Date.now();
      const last = previous;
      previous = { stats, at };
      if (last === null) {
        return; // the first sample only sets the baseline
      }
      if (KEYS.some((key) => stats[key] < last.stats[key])) {
        setRates(zeroRates());
        setSeries(zeroSeries());
        return; // this sample becomes the new session's baseline
      }
      const seconds = Math.max((at - last.at) / 1000, 0.001);
      const step = zeroRates();
      for (const key of KEYS) {
        step[key] = Math.max(0, stats[key] - last.stats[key]) / seconds;
      }
      setRates(step);
      setSeries((current) => {
        const next = emptySeries();
        for (const key of KEYS) {
          next[key] = [...current[key].slice(1), step[key]];
        }
        return next;
      });
    }).then((stop) => {
      if (disposed) {
        stop();
      } else {
        unlisten = stop;
      }
    });

    onCleanup(() => {
      disposed = true;
      unlisten?.();
    });
  });

  return <TrafficStatsContext.Provider value={{ rates, series }}>{props.children}</TrafficStatsContext.Provider>;
};

export function useTrafficStats(): TrafficStatsStore {
  const store = useContext(TrafficStatsContext);
  if (!store) {
    throw new Error("useTrafficStats must be used within TrafficStatsProvider");
  }
  return store;
}
