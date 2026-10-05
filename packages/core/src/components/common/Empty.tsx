import type { Component } from "solid-js";
import { Show } from "solid-js";
import { Dynamic } from "solid-js/web";

interface Props {
  icon: Component<{ size?: number | string; class?: string }>;
  title: string;
  description?: string;
  action?: Component;
}

export const Empty: Component<Props> = (props) => (
  <div class="flex flex-col items-center gap-3 rounded-box border border-dashed border-base-300 px-6 py-16 text-center">
    <props.icon size={48} class="opacity-20" />
    <div class="space-y-1">
      <p class="font-semibold">{props.title}</p>
      <Show when={props.description}>
        <p class="text-sm text-base-content/60">{props.description}</p>
      </Show>
    </div>
    <Dynamic component={props.action} />
  </div>
);
