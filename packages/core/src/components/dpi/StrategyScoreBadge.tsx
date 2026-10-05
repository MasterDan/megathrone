import type { Component } from "solid-js";
import { createMemo, createSignal, onCleanup } from "solid-js";

import { Modal } from "@/components/common/daisy-ui/Modal";
import { Popover } from "@/components/common/daisy-ui/Popover";
import { DpiSiteChecksList } from "@/components/dpi/DpiSiteChecksList";
import type { DpiStrategy } from "@/types";

/** Popovers only make sense with a real hover pointer; touch devices get the
 *  modal instead (a tap fires emulated mouseenter too, so gate on this). */
const CAN_HOVER =
  typeof window !== "undefined" &&
  window.matchMedia("(hover: hover) and (pointer: fine)").matches;

const HOVER_DELAY_MS = 120;
const HOVER_HIDE_DELAY_MS = 200;

interface Props {
  strategy: DpiStrategy;
}

/** The test-site pass rate on a strategy row: a small percentage badge.
 *  Hover opens a lazy popover (desktop); click/tap opens a modal (mobile
 *  and keyboard-friendly fallback). Hidden until the strategy has been
 *  tested at least once. */
export const StrategyScoreBadge: Component<Props> = (props) => {
  const [popoverOpen, setPopoverOpen] = createSignal(false);
  const [modalOpen, setModalOpen] = createSignal(false);
  let hoverTimer: number | undefined;
  onCleanup(() => window.clearTimeout(hoverTimer));

  const percent = createMemo(() =>
    props.strategy.urlTotal > 0
      ? Math.round((props.strategy.urlOk / props.strategy.urlTotal) * 100)
      : 0,
  );
  const tone = createMemo(() =>
    percent() === 100 ? "badge-success" : percent() === 0 ? "badge-error" : "badge-warning",
  );

  const scheduleShow = () => {
    if (!CAN_HOVER) {
      return;
    }
    window.clearTimeout(hoverTimer);
    hoverTimer = window.setTimeout(() => setPopoverOpen(true), HOVER_DELAY_MS);
  };
  // The panel is portaled, so moving the pointer from the badge into it
  // counts as "leaving" — the hide is delayed and cancelled by the panel
  // content's own mouseenter.
  const scheduleHide = () => {
    window.clearTimeout(hoverTimer);
    hoverTimer = window.setTimeout(() => setPopoverOpen(false), HOVER_HIDE_DELAY_MS);
  };
  const cancelHover = () => window.clearTimeout(hoverTimer);

  return (
    <>
      <Popover
        open={popoverOpen()}
        onOpenChange={setPopoverOpen}
        contentClass="w-64 max-h-72 overflow-y-auto p-3"
        activator={(api) => (
          <span
            ref={api.ref}
            class="relative inline-flex"
            onMouseEnter={scheduleShow}
            onMouseLeave={scheduleHide}
          >
            <button
              type="button"
              class={`badge badge-outline badge-sm shrink-0 cursor-help ${tone()}`}
              title="Test site availability — hover for details"
              aria-label={`Test site availability ${percent()} percent`}
              onClick={(event) => {
                event.stopPropagation();
                cancelHover();
                setPopoverOpen(false);
                setModalOpen(true);
              }}
            >
              {percent()}%
            </button>
          </span>
        )}
      >
        <div onMouseEnter={cancelHover} onMouseLeave={scheduleHide}>
          <p class="mb-2 text-[11px] font-semibold tracking-wide text-base-content/50 uppercase">
            Site checks · {props.strategy.urlOk}/{props.strategy.urlTotal}
          </p>
          <DpiSiteChecksList strategyId={props.strategy.id} />
        </div>
      </Popover>
      <Modal
        opened={modalOpen}
        setOpened={setModalOpen}
        title={<span title={props.strategy.name}>{props.strategy.name}</span>}
        class="max-w-sm"
      >
        <p class="mb-3 text-xs text-base-content/50">
          Test site availability through this strategy · {props.strategy.urlOk}/
          {props.strategy.urlTotal}
        </p>
        <DpiSiteChecksList strategyId={props.strategy.id} />
      </Modal>
    </>
  );
};
