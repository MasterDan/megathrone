import clsx from "clsx";
import { For } from "solid-js";

export interface OptionTabItem<T extends string> {
  value: T;
  label: string;
}

/**
 * A group of tab-like buttons in the dock style (rounded ghost buttons, the
 * active one gets a soft fill). A compact replacement for selects/comboboxes.
 */
export function OptionTabs<T extends string>(props: {
  items: Array<OptionTabItem<T>>;
  value: T;
  onChange: (value: T) => void;
  disabled?: boolean;
  class?: string;
}) {
  return (
    <div role="tablist" class={clsx("flex flex-wrap gap-1", props.class)}>
      <For each={props.items}>
        {(item) => (
          <button
            type="button"
            role="tab"
            aria-selected={props.value === item.value}
            class="btn btn-ghost btn-sm rounded-xl"
            classList={{
              "bg-base-content/10 text-base-content": props.value === item.value,
            }}
            disabled={props.disabled}
            onClick={() => props.onChange(item.value)}
          >
            {item.label}
          </button>
        )}
      </For>
    </div>
  );
}
