import type { Component } from "solid-js";
import { createEffect, onCleanup, onMount } from "solid-js";
import { createResizeObserver } from "@solid-primitives/resize-observer";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

export interface TrafficSeriesLine {
  values: number[];
  color: string;
}

/** Idle-scale ceiling until real traffic lifts the y range. */
const MIN_CEILING = 1024;

/** Smallest round 1/2/5 × 10^k ceiling ≥ value — the top edge of the chart
 *  stays a stable, readable number. */
export const niceCeiling = (value: number): number => {
  if (!Number.isFinite(value) || value <= MIN_CEILING) {
    return MIN_CEILING;
  }
  const scale = 10 ** Math.floor(Math.log10(value));
  const scaled = value / scale;
  const nice = scaled <= 1 ? 1 : scaled <= 2 ? 2 : scaled <= 5 ? 5 : 10;
  return nice * scale;
};

/** The y-axis ceiling a chart of these lines settles on (for the scale
 *  label rendered outside the plot). */
export const seriesCeiling = (lines: readonly TrafficSeriesLine[]): number =>
  niceCeiling(
    lines.reduce((peak, line) => line.values.reduce((top, value) => Math.max(top, value), peak), 0),
  );

/** "#rrggbb" as an rgba() string. */
const rgba = (hex: string, alpha: number): string => {
  const value = hex.replace("#", "");
  return `rgba(${parseInt(value.slice(0, 2), 16)}, ${parseInt(value.slice(2, 4), 16)}, ${parseInt(
    value.slice(4, 6),
    16,
  )}, ${alpha})`;
};

/** Vertical color→transparent gradient under a curve — rebuilt per frame
 *  against the current plot box (uPlot calls the hook while drawing). */
const areaGradient = (self: uPlot, color: string): CanvasGradient => {
  const gradient = self.ctx.createLinearGradient(0, self.bbox.top, 0, self.bbox.top + self.bbox.height);
  gradient.addColorStop(0, rgba(color, 0.32));
  gradient.addColorStop(0.6, rgba(color, 0.08));
  gradient.addColorStop(1, rgba(color, 0));
  return gradient;
};

/**
 * One rate chart on uPlot (the lightest full-blown plotting lib around —
 * canvas, no deps). Stripped to the curves: no axes, legend or cursor —
 * the dock draws its own translucent panel, gridlines and labels around
 * the transparent canvas. `lines` carries static colors; the values are
 * pushed in reactively via `setData`.
 */
export const TrafficChart: Component<{
  lines: () => readonly TrafficSeriesLine[];
  /** fixed x length — every series is zero-padded to it */
  slots: number;
}> = (props) => {
  let mount!: HTMLDivElement;

  onMount(() => {
    const xs = Array.from({ length: props.slots }, (_, index) => index);
    const series: uPlot.Series[] = [
      {},
      ...props.lines().map((line) => ({
        stroke: line.color,
        width: 1.5,
        fill: (self: uPlot) => areaGradient(self, line.color),
        spanGaps: true,
        points: { show: false },
      })),
    ];

    const plot = new uPlot(
      {
        width: Math.max(mount.clientWidth, 50),
        height: Math.max(mount.clientHeight, 30),
        series,
        axes: [{ show: false }, { show: false }],
        scales: { y: { range: (_self, min, max) => [Math.min(min, 0), niceCeiling(max)] } },
        cursor: { show: false },
        legend: { show: false },
      },
      [xs, ...props.lines().map((line) => line.values)],
      mount,
    );

    createEffect(() => {
      const lines = props.lines();
      plot.setData([xs, ...lines.map((line) => line.values)]);
    });

    createResizeObserver(mount, ({ width, height }) => {
      if (width > 20 && height > 20) {
        plot.setSize({ width, height });
      }
    });

    onCleanup(() => plot.destroy());
  });

  return <div ref={mount} class="h-full w-full" />;
};
