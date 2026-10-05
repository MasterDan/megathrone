import type { Accessor, Component } from "solid-js";
import { Show } from "solid-js";

import { CrownMark } from "@/components/common/CrownMark";

export const ConnectButton: Component<{
  connected: Accessor<boolean>;
  connecting: Accessor<boolean>;
  disabled: Accessor<boolean>;
  onClick: () => void;
}> = (props) => (
  <button
    type="button"
    title={props.connected() ? "Disconnect" : "Connect"}
    aria-label={props.connected() ? "Disconnect" : "Connect"}
    aria-pressed={props.connected()}
    disabled={props.disabled() || props.connecting()}
    onClick={props.onClick}
    class="group flex size-64 items-center justify-center rounded-full bg-gradient-to-br shadow-xl ring-4 transition-all active:scale-95 disabled:pointer-events-none disabled:opacity-40 disabled:active:scale-100"
    classList={{
      "from-violet-500 to-violet-700 shadow-violet-700/30 ring-violet-500/20 hover:from-violet-500 hover:to-violet-600":
        !props.connected(),
      "from-emerald-500 to-emerald-700 shadow-emerald-700/30 ring-emerald-500/25": props.connected(),
    }}
  >
    <Show
      when={!props.connecting()}
      fallback={<span class="loading loading-spinner size-20 text-white" />}
    >
      <CrownMark class="size-36 text-white transition-transform duration-300 group-hover:scale-105" />
    </Show>
  </button>
);
