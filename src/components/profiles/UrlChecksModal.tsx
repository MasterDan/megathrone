import type { Accessor, Component } from "solid-js";
import { Show } from "solid-js";

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
}

/** Per-URL deep-probe details of one endpoint as a bottom sheet; opened by
 *  the score badge, or by the test button once its run finishes. */
export const UrlChecksModal: Component<Props> = (props) => {
  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title={props.item?.tag}
      class="max-w-sm"
    >
      <p class="mb-3 text-xs text-base-content/50">
        Category URL availability through this endpoint
      </p>
      <Show when={props.error}>
        <div class="alert alert-error mb-3 py-2 text-xs">
          <span class="break-all">{props.error}</span>
        </div>
      </Show>
      <Show when={props.item}>
        {(item) => <UrlChecksList endpointId={item().id} />}
      </Show>
    </Modal>
  );
};
