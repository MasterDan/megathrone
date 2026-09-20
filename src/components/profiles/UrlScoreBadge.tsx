import type { Component } from "solid-js";
import { Show, createMemo, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { TbOutlineFlask } from "solid-icons/tb";

import type { EndpointItem } from "@/types";

interface Props {
  item: EndpointItem;
  /** Opens the per-URL details modal — owned above the grid (ProfileItems),
   *  so it survives card re-renders while a scan rebuilds the list. */
  onShowChecks: (item: EndpointItem, error?: string | null) => void;
}

/** The custom-test-URL pass rate on an endpoint card: a small percentage
 *  badge. Click opens the per-URL details as a modal. */
export const UrlScoreBadge: Component<Props> = (props) => {
  const percent = createMemo(() =>
    props.item.urlTotal > 0 ? Math.round((props.item.urlOk / props.item.urlTotal) * 100) : 0,
  );
  const tone = createMemo(() =>
    percent() === 100 ? "badge-success" : percent() === 0 ? "badge-error" : "badge-warning",
  );

  return (
    <button
      type="button"
      class={`badge badge-outline badge-sm shrink-0 cursor-pointer ${tone()}`}
      title="Category URL availability — click for details"
      aria-label={`Category URL availability ${percent()} percent`}
      onClick={(event) => {
        event.stopPropagation();
        props.onShowChecks(props.item);
      }}
    >
      {percent()}%
    </button>
  );
};

/** Shown instead of the score badge while the endpoint has no deep-probe
 *  results: runs the category URL test on this single endpoint on demand,
 *  then opens the details (or the error) as a modal. */
export const UrlTestButton: Component<Props> = (props) => {
  const [running, setRunning] = createSignal(false);

  const run = async () => {
    if (running()) {
      return;
    }
    setRunning(true);
    let failed: string | null = null;
    try {
      await invoke("profile_test_endpoint_urls", { itemId: props.item.id });
    } catch (cause) {
      failed = String(cause);
    } finally {
      setRunning(false);
      props.onShowChecks(props.item, failed);
    }
  };

  return (
    <button
      type="button"
      class="badge badge-outline badge-sm shrink-0 cursor-pointer gap-0.5 text-base-content/60"
      title="Run the category URL test on this endpoint"
      aria-label="Run the category URL test on this endpoint"
      disabled={running()}
      onClick={(event) => {
        event.stopPropagation();
        void run();
      }}
    >
      <Show
        when={!running()}
        fallback={<span class="loading loading-spinner block" style={{ width: "10px", height: "10px" }} />}
      >
        <TbOutlineFlask size={12} />
        test
      </Show>
    </button>
  );
};
