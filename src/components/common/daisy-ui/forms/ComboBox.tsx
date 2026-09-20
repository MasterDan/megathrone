import { useDebounceValue } from "@elysiumorg/solid-use";
import { TbOutlineChevronDown, TbOutlineX } from "solid-icons/tb";
import { clsx } from "clsx";
import {
  createMemo,
  createResource,
  createSignal,
  For,
  Show,
  type Accessor,
  type JSXElement,
} from "solid-js";
import { Popover } from "../Popover";

/** List-row API for the `children` render prop. */
export interface ComboBoxItemApi {
  selected: Accessor<boolean>;
  select: () => void;
}

/** Chip API for the `displayItem` render prop. */
export interface ComboBoxValueApi {
  clear: () => void;
}

export type ComboBoxChildren<TItem> = (
  item: TItem,
  api: ComboBoxItemApi,
) => JSXElement;

export type ComboBoxDisplayItem<TItem> = (
  item: TItem,
  api: ComboBoxValueApi,
) => JSXElement;

interface ComboBoxBaseProps<TItem> {
  /**
   * Data source: takes the search query, returns an array — right away or as
   * a Promise. The component turns it into a resource and spins the loader;
   * typing works without waiting for the result. Reactive values read inside
   * `search` (e.g. another resource holding the model list) are tracked:
   * changing them restarts the search even without a new query.
   */
  search: (query: string) => TItem[] | Promise<TItem[]>;
  /** Item identifier. Needed for highlighting/selection (async especially). */
  getKey: (item: TItem) => string | number;
  /** Text representation of an item (for the default chip). */
  displayValue?: (item: TItem) => string;
  /** Custom rendering of the selected value (chip). Fallback — a text chip. */
  displayItem?: ComboBoxDisplayItem<TItem>;
  placeholder?: string;
  /** Request delay in ms (for async search). 0 — no delay. */
  debounce?: number;
  /** Close the popover after a selection. Single: default true, Multi: false. */
  closeOnSelect?: boolean;
  /** Clear the search query after a selection (default true). */
  clearOnSelect?: boolean;
  /** Disable the control entirely. */
  disabled?: boolean;
  class?: string;
  children: ComboBoxChildren<TItem>;
}

export type ComboBoxProps<TItem> =
  | (ComboBoxBaseProps<TItem> & {
      multiple?: false;
      value?: NoInfer<TItem>;
      onChange?: (value: NoInfer<TItem> | undefined) => void;
    })
  | (ComboBoxBaseProps<TItem> & {
      multiple: true;
      value?: NoInfer<TItem[]>;
      onChange?: (value: NoInfer<TItem[]>) => void;
    });

export function ComboBox<TItem>(props: ComboBoxProps<TItem>): JSXElement {
  const multiple = (): boolean => props.multiple === true;
  const isControlled = (): boolean => props.onChange !== undefined;

  // Internally the selection is always an array. In controlled mode `sel()`
  // is a derived accessor straight off props.value (no copying signal/effect)
  // so toggle/removeItem always see the up-to-date set.
  const [internal, setInternal] = createSignal<TItem[]>([]);
  const sel = (): TItem[] => {
    if (!isControlled()) return internal();
    const v = props.value;
    return multiple()
      ? Array.isArray(v)
        ? (v as TItem[])
        : []
      : v != null
        ? [v as TItem]
        : [];
  };
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");

  const getKey = (item: TItem) => props.getKey(item);

  const emit = (next: TItem[]) => {
    const onChange = props.onChange as ((v: unknown) => void) | undefined;
    if (isControlled()) onChange?.(multiple() ? next : next[0]);
    else setInternal(next);
  };

  const toggle = (item: TItem) => {
    const key = getKey(item);
    const prev = sel();
    const idx = prev.findIndex((i) => getKey(i) === key);
    if (multiple()) {
      const next =
        idx >= 0 ? prev.filter((_, i) => i !== idx) : [...prev, item];
      emit(next);
      if (props.clearOnSelect !== false) setQuery("");
      if (props.closeOnSelect === true) setOpen(false);
    } else {
      if (idx < 0) emit([item]);
      if (props.closeOnSelect !== false) setOpen(false);
      if (props.clearOnSelect !== false) setQuery("");
    }
  };

  const removeItem = (item: TItem) =>
    queueMicrotask(() => emit(sel().filter((i) => getKey(i) !== getKey(item))));

  const isSelected = (item: TItem) =>
    sel().some((s) => getKey(s) === getKey(item));

  // Resource source: with debounce (async) or direct (sync).
  const source: Accessor<string> =
    props.debounce && props.debounce > 0
      ? useDebounceValue(query, props.debounce)
      : query;

  // The search runs in a tracked scope (createMemo) and its result becomes
  // the resource source. createResource calls the fetcher untracked, so it
  // does not react to state read inside `search` (e.g. the provider's models
  // resource loading after mount) — without the memo the empty first result
  // would stay cached until the query changes.
  const searched = createMemo(() => props.search(source()));
  const [items] = createResource(searched, (result) => result);

  let inputRef: HTMLInputElement | undefined;

  return (
    <Popover
      open={open()}
      onOpenChange={setOpen}
      matchActivatorWidth
      activator={(api) => (
        <div
          ref={api.ref}
          class={clsx(
            "input input-md relative flex w-full items-center flex-wrap gap-1 pr-9",
            props.disabled ? "input-disabled cursor-not-allowed" : "cursor-text",
            props.class,
          )}
          onMouseDown={(e) => {
            if (props.disabled) return;
            if (!(e.target as HTMLElement).closest("button")) {
              e.preventDefault();
              inputRef?.focus();
              api.show();
            }
          }}
        >
          <For each={sel()}>
            {(item) => (
              <Show
                when={props.displayItem}
                fallback={
                  <span class="inline-flex items-center gap-1 rounded-xl bg-base-300 px-1.5 h-5 text-xs">
                    <Show when={props.displayValue} fallback={String(item)}>
                      {props.displayValue!(item)}
                    </Show>
                    <button
                      type="button"
                      class="flex size-4 cursor-pointer items-center justify-center rounded-full transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
                      disabled={props.disabled}
                      onMouseDown={(e) => {
                        e.preventDefault();
                        e.stopPropagation();
                      }}
                      onClick={(e) => {
                        e.preventDefault();
                        e.stopPropagation();
                        removeItem(item);
                      }}
                    >
                      <TbOutlineX size={12} />
                    </button>
                  </span>
                }
              >
                {props.displayItem!(item, { clear: () => removeItem(item) })}
              </Show>
            )}
          </For>
          <input
            ref={inputRef}
            type="text"
            class="min-w-[6ch] flex-1 border-0 bg-transparent p-0 outline-none"
            disabled={props.disabled}
            placeholder={sel().length === 0 ? props.placeholder : ""}
            value={query()}
            onInput={(e) => {
              setQuery(e.currentTarget.value);
              api.show();
            }}
            onFocus={() => api.show()}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                // stopPropagation: only this popover closes, not the modal
                // it may sit in (the window-level overlay-escape listener).
                e.stopPropagation();
                api.hide();
              }
              if (e.key === "ArrowDown") {
                e.preventDefault();
                api.show();
              }
              if (
                e.key === "Backspace" &&
                e.currentTarget.value === "" &&
                sel().length > 0
              ) {
                e.preventDefault();
                removeItem(sel()[sel().length - 1]);
              }
            }}
          />
          <span class="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2">
            <Show
              when={!items.loading}
              fallback={<span class="loading loading-spinner loading-xs" />}
            >
              <TbOutlineChevronDown size={16} class="opacity-70" />
            </Show>
          </span>
        </div>
      )}
    >
      <div class="max-h-72 overflow-auto p-1">
        <Show
          when={!items.loading}
          fallback={
            <div class="flex items-center justify-center gap-2 py-6 text-sm opacity-70">
              <span class="loading loading-spinner loading-sm" />
              Loading…
            </div>
          }
        >
          <Show
            when={(items() ?? []).length > 0}
            fallback={
              <div class="py-6 text-center text-sm opacity-70">
                Nothing found
              </div>
            }
          >
            <For each={items()}>
              {(item) =>
                props.children(item, {
                  selected: () => isSelected(item),
                  select: () => toggle(item),
                })
              }
            </For>
          </Show>
        </Show>
      </div>
    </Popover>
  );
}
