import type { Accessor, Component } from "solid-js";
import { Show, createResource, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { TbOutlineCheck, TbOutlineCopy } from "solid-icons/tb";

import { Modal } from "@/components/common/daisy-ui/Modal";
import { PROTOCOL_BADGES } from "@/components/profiles/EndpointCard";
import type { EndpointItem, ItemDetail } from "@/types";

interface Props {
  item: EndpointItem | null;
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
}

function prettyJson(value: string) {
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}

export const ItemDetailModal: Component<Props> = (props) => {
  // The modal stays mounted for exit animations, so the detail is fetched
  // only while it is actually open.
  const [detail] = createResource(
    () => (props.opened() ? props.item?.id ?? null : null),
    (itemId) =>
      itemId === null
        ? Promise.resolve(null)
        : invoke<ItemDetail>("profile_item_detail", { itemId }),
  );

  const rawValue = () => detail()?.raw ?? "";
  const jsonValue = () => prettyJson(detail()?.outboundJson ?? "");
  const [copied, setCopied] = createSignal<"raw" | "json" | null>(null);

  const copy = async (what: "raw" | "json") => {
    const value = what === "raw" ? rawValue() : jsonValue();
    if (!value) {
      return;
    }
    await navigator.clipboard.writeText(value);
    setCopied(what);
    setTimeout(() => setCopied(null), 1500);
  };

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title={
        <span class="truncate" title={props.item?.tag}>
          {props.item?.tag}
        </span>
      }
      class="max-w-lg"
    >
      <Show when={props.item} keyed>
        {(item) => (
          <div class="space-y-4">
            <div class="flex items-start justify-between gap-2">
              <p class="min-w-0 truncate text-xs text-base-content/50">
                {item.server}:{item.serverPort}
              </p>
              <span
                class={`badge badge-sm shrink-0 ${PROTOCOL_BADGES[item.protocol] ?? "badge-ghost"}`}
              >
                {item.protocol}
              </span>
            </div>

            <Show
              when={!detail.loading}
              fallback={
                <div class="flex justify-center py-6">
                  <span class="loading loading-spinner" />
                </div>
              }
            >
              <Show when={detail.error}>
                <div class="alert alert-error py-2 text-sm">{String(detail.error)}</div>
              </Show>

              <section class="space-y-1">
                <div class="flex items-center justify-between">
                  <h4 class="text-xs font-semibold uppercase tracking-wide text-base-content/50">
                    sing-box outbound
                  </h4>
                  <button
                    type="button"
                    class="inline-flex h-6 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-base-content/10 px-2.5 text-xs font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20"
                    onClick={() => void copy("json")}
                  >
                    <Show when={copied() === "json"} fallback={<TbOutlineCopy size={14} />}>
                      <TbOutlineCheck size={14} />
                    </Show>
                    JSON
                  </button>
                </div>
                <pre class="max-h-60 overflow-auto rounded-lg bg-base-200 p-2 text-[11px] leading-relaxed">
                  {jsonValue()}
                </pre>
              </section>

              <section class="space-y-1">
                <div class="flex items-center justify-between">
                  <h4 class="text-xs font-semibold uppercase tracking-wide text-base-content/50">
                    Raw link
                  </h4>
                  <button
                    type="button"
                    class="inline-flex h-6 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-base-content/10 px-2.5 text-xs font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20"
                    onClick={() => void copy("raw")}
                  >
                    <Show when={copied() === "raw"} fallback={<TbOutlineCopy size={14} />}>
                      <TbOutlineCheck size={14} />
                    </Show>
                    Copy
                  </button>
                </div>
                <pre class="max-h-32 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-base-200 p-2 text-[11px] leading-relaxed">
                  {rawValue()}
                </pre>
              </section>
            </Show>
          </div>
        )}
      </Show>
    </Modal>
  );
};
