import type { Component } from "solid-js";
import { Dynamic } from "solid-js/web";
import { For } from "solid-js";
import { TbOutlineArrowDown, TbOutlineArrowUp } from "solid-icons/tb";

import { TrafficChart, seriesCeiling } from "@/components/home/TrafficChart";
import type { TrafficSeriesLine } from "@/components/home/TrafficChart";
import { TRAFFIC_WINDOW, useTrafficStats } from "@/hooks/data/useTrafficStats";
import type { TrafficKey } from "@/hooks/data/useTrafficStats";
import { formatRate } from "@/utils/format";

/** Bucket palette: proxy keeps the app's violet, DPI the connected-emerald,
 *  direct gets the cool info-sky. */
const BUCKETS: ReadonlyArray<{
  name: string;
  color: string;
  upKey: TrafficKey;
  downKey: TrafficKey;
}> = [
  { name: "Proxy", color: "#8b5cf6", upKey: "proxyUp", downKey: "proxyDown" },
  { name: "DPI", color: "#10b981", upKey: "dpiUp", downKey: "dpiDown" },
  { name: "Direct", color: "#0ea5e9", upKey: "directUp", downKey: "directDown" },
];

interface BucketRate {
  name: string;
  color: string;
  rate: number;
}

/**
 * One chart panel: the curves fill the whole card, while the readings live
 * on overlays — label top-left, current total + scale ceiling top-right,
 * and the per-bucket rate legend floating on a scrim along the bottom.
 */
const ChartBlock: Component<{
  label: string;
  icon: Component<{ class?: string }>;
  lines: () => readonly TrafficSeriesLine[];
  total: () => number;
  buckets: () => readonly BucketRate[];
}> = (props) => {
  const ceiling = () => seriesCeiling(props.lines());
  return (
    <div class="relative min-h-0 flex-1 overflow-hidden rounded-xl border border-base-content/10 bg-base-content/5">
      <For each={[0.25, 0.5, 0.75]}>
        {(grid) => (
          <span
            class="pointer-events-none absolute inset-x-0 h-px bg-base-content/10"
            style={{ top: `${grid * 100}%` }}
            aria-hidden="true"
          />
        )}
      </For>
      {/* y-axis: the rate each gridline stands for, so a past bump is
          readable even after the live total drops back to zero */}
      <For each={[0.25, 0.5, 0.75]}>
        {(grid) => (
          <span
            class="pointer-events-none absolute left-1.5 z-10 -translate-y-1/2 rounded bg-base-100/80 px-0.5 text-[9px] leading-none tabular-nums text-base-content/40"
            style={{ top: `${grid * 100}%` }}
          >
            {formatRate(ceiling() * grid)}
          </span>
        )}
      </For>
      <TrafficChart lines={props.lines} slots={TRAFFIC_WINDOW} />
      <div class="pointer-events-none absolute inset-x-0 top-0 z-10 flex items-start justify-between gap-2 p-2.5">
        <span class="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-widest text-base-content/50">
          <Dynamic component={props.icon} class="size-3.5" />
          {props.label}
        </span>
        <span class="flex flex-col items-end leading-tight">
          <span class="text-sm font-semibold tabular-nums text-base-content/90">
            {formatRate(props.total())}
          </span>
          <span class="text-[10px] tabular-nums text-base-content/35">
            max {formatRate(ceiling())}
          </span>
        </span>
      </div>
      <div class="pointer-events-none absolute inset-x-0 bottom-0 z-10 flex items-center justify-between gap-3 bg-gradient-to-t from-base-100/90 via-base-100/50 to-transparent px-2.5 pb-1.5 pt-5 text-[10px] tabular-nums text-base-content/60">
        <For each={props.buckets()}>
          {(bucket) => (
            <span class="flex items-center gap-1 whitespace-nowrap">
              <span class="size-1.5 rounded-full" style={{ background: bucket.color }} />
              <span class="text-base-content/40">{bucket.name}</span>
              {formatRate(bucket.rate)}
            </span>
          )}
        </For>
      </div>
    </div>
  );
};

/**
 * The dock that replaces the mode controls while connected: live up/down
 * rate charts for the session's three outbounds (proxy / DPI tunnel /
 * direct) over a translucent panel — fed by `useTrafficStats`. All text
 * (labels, totals, the per-bucket legend) is overlaid on the charts, so
 * the curves get the full card height.
 */
export const TrafficDock: Component = () => {
  const { rates, series } = useTrafficStats();

  const upLines = () =>
    BUCKETS.map((bucket) => ({ values: series()[bucket.upKey], color: bucket.color }));
  const downLines = () =>
    BUCKETS.map((bucket) => ({ values: series()[bucket.downKey], color: bucket.color }));
  const totalUp = () => BUCKETS.reduce((sum, bucket) => sum + rates()[bucket.upKey], 0);
  const totalDown = () => BUCKETS.reduce((sum, bucket) => sum + rates()[bucket.downKey], 0);
  const upBuckets = () =>
    BUCKETS.map((bucket) => ({ name: bucket.name, color: bucket.color, rate: rates()[bucket.upKey] }));
  const downBuckets = () =>
    BUCKETS.map((bucket) => ({
      name: bucket.name,
      color: bucket.color,
      rate: rates()[bucket.downKey],
    }));

  return (
    <div class="grid h-full w-full grid-cols-2 gap-3 rounded-2xl border border-base-content/10 bg-base-100/60 p-4 shadow-lg backdrop-blur-md">
      <ChartBlock label="Upload" icon={TbOutlineArrowUp} lines={upLines} total={totalUp} buckets={upBuckets} />
      <ChartBlock
        label="Download"
        icon={TbOutlineArrowDown}
        lines={downLines}
        total={totalDown}
        buckets={downBuckets}
      />
    </div>
  );
};
