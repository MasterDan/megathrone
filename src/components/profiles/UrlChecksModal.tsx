import type { Accessor, Component } from "solid-js";
import { Show, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { TbOutlineRefresh } from "solid-icons/tb";

import { Modal } from "@/components/common/daisy-ui/Modal";
import type { EndpointItem } from "@/types";
import { UrlChecksList } from "./UrlChecksList";

interface Props {
  /** Null while closed — the modal itself stays mounted (it is owned above
   *  the grid and outlives card re-renders). */
  item: EndpointItem | null;
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
  /** Set when the on-demand endpoint test failed — shown above the list. */
  error?: string | null;
  /** Outcome of a retest started here — keeps the parent's error state the
   *  single source of truth (a success clears its stale error). */
  onResult?: (error: string | null) => void;
}

/** Per-URL deep-probe details of one endpoint as a bottom sheet; opened by
 *  the score badge, or by the test button once its run finishes. The refresh
 *  button re-runs the same on-demand test and refreshes the list in place. */
export const UrlChecksModal: Component<Props> = (props) => {
  const [retesting, setRetesting] = createSignal(false);
  const [token, setToken] = createSignal(0);

  const retest = async () => {
    const current = props.item;
    if (!current || retesting()) {
      return;
    }
    setRetesting(true);
    let failed: string | null = null;
    try {
      await invoke("profile_test_endpoint_urls", { itemId: current.id });
    } catch (cause) {
      failed = String(cause);
    }
    setRetesting(false);
    // the modal may have been closed or re-opened on another endpoint meanwhile
    if (props.item?.id === current.id) {
      if (!failed) {
        setToken((value) => value + 1);
      }
      props.onResult?.(failed);
    }
  };

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title={props.item?.tag}
      class="max-w-sm"
    >
      <div class="mb-3 flex items-center justify-between gap-2">
        <p class="text-xs text-base-content/50">
          Category URL availability through this endpoint
        </p>
        <button
          type="button"
          class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
          title="Re-run the category URL test on this endpoint"
          aria-label="Re-run the category URL test on this endpoint"
          disabled={retesting()}
          onClick={() => void retest()}
        >
          <Show
            when={!retesting()}
            fallback={
              <span class="loading loading-spinner block" style={{ width: "14px", height: "14px" }} />
            }
          >
            <TbOutlineRefresh size={14} />
          </Show>
        </button>
      </div>
      <Show when={props.error}>
        <div class="alert alert-error mb-3 py-2 text-xs">
          <span class="break-all">{props.error}</span>
        </div>
      </Show>
      <Show when={props.item}>
        {(item) => <UrlChecksList endpointId={item().id} refreshToken={token()} />}
      </Show>
    </Modal>
  );
};
