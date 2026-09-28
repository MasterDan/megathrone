import type { Accessor, Component } from "solid-js";
import { For, Show } from "solid-js";
import { Dynamic } from "solid-js/web";
import { TbOutlineNetwork, TbOutlinePower, TbOutlineWorld } from "solid-icons/tb";

import { BUILD_UI_VARIANT } from "@/platform";
import type { ProxyMode } from "@/types";

/** One of three modes is active at a time — a segmented control rather than
 *  independent toggles, since TUN is a superset of System Proxy. Styled
 *  after the floating top dock (AppLayout).
 *
 *  Neither takeover mode is reachable on Android: TUN needs a VpnService
 *  tunnel and the global system proxy is a privileged setting — the mobile
 *  build shows neither (the backend refuses them with an explanatory error
 *  either way) and the local proxy port is the only integration point. */
const MODES: ReadonlyArray<{ value: ProxyMode; label: string; icon: Component<{ class?: string }> }> = [
  { value: "off", label: "Off", icon: TbOutlinePower },
  ...(BUILD_UI_VARIANT === "desktop"
    ? [
        { value: "system-proxy" as const, label: "System Proxy", icon: TbOutlineWorld },
        { value: "tun" as const, label: "TUN", icon: TbOutlineNetwork },
      ]
    : []),
];

export const ModeControls: Component<{
  mode: Accessor<ProxyMode>;
  onChange: (mode: ProxyMode) => void;
  /** While a connection is up the mode is baked into the running instance —
   *  changing it would not apply until a reconnect. */
  disabled?: Accessor<boolean>;
}> = (props) => {
  const disabled = () => props.disabled?.() ?? false;

  return (
    <Show when={MODES.length > 1}>
      <div class="flex items-center gap-1 rounded-2xl border border-base-content/10 bg-base-100/60 p-1 shadow-lg backdrop-blur-md">
        <div role="group" aria-label="Proxy mode" class="flex items-center gap-1">
          <For each={MODES}>
            {(option) => (
              <button
                type="button"
                aria-pressed={props.mode() === option.value}
                class="btn btn-ghost btn-sm gap-2 rounded-xl"
                classList={{
                  "text-base-content/60": props.mode() !== option.value,
                  "bg-base-content/10 text-base-content": props.mode() === option.value,
                }}
                disabled={disabled()}
                onClick={() => props.onChange(option.value)}
              >
                <Dynamic component={option.icon} class="size-4" />
                {option.label}
              </button>
            )}
          </For>
        </div>
      </div>
    </Show>
  );
};
