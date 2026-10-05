import type { Accessor, Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";

import { Button } from "@/components/common/daisy-ui/Button";
import { Modal } from "@/components/common/daisy-ui/Modal";
import type { TestSiteCategory } from "@/types";

interface Props {
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
  busy: Accessor<boolean>;
  error: Accessor<string | null>;
  /** The category being renamed; null — add mode. */
  category: Accessor<TestSiteCategory | null>;
  /** Creates or renames; resolves true on success (the modal closes). */
  onSubmit: (name: string) => Promise<boolean>;
}

/**
 * Name prompt for URL categories (Settings → Routing): add mode from the
 * Routing list, rename mode from the category page header — both as a
 * modal, so the header layouts never shift.
 */
export const CategoryModal: Component<Props> = (props) => {
  const [name, setName] = createSignal("");

  // The modal stays mounted for exit animations, so drafts reset on open —
  // prefilled from the category in rename mode.
  createEffect(() => {
    if (!props.opened()) {
      return;
    }
    setName(props.category()?.name ?? "");
  });

  const canSubmit = () => {
    const trimmed = name().trim();
    if (props.busy() || !trimmed) {
      return false;
    }
    const editing = props.category();
    return !editing || trimmed !== editing.name;
  };

  const close = () => props.setOpened(false);

  const submit = () => {
    if (!canSubmit()) {
      return;
    }
    void props.onSubmit(name().trim()).then((ok) => {
      if (ok) {
        close();
      }
    });
  };

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title={props.category() ? "Rename category" : "Add category"}
      class="max-w-md"
      actions={
        <>
          <Button ghost disabled={props.busy()} onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!canSubmit()} onClick={submit}>
            {props.category() ? "Save" : "Add"}
          </Button>
        </>
      }
    >
      <div class="space-y-4">
        <label class="block">
          <span class="label-text mb-1 block text-sm">Name</span>
          <input
            type="text"
            class="input input-bordered w-full"
            placeholder="e.g. Streaming"
            value={name()}
            ref={(el) =>
              queueMicrotask(() => {
                el.focus();
                if (props.category()) {
                  el.select();
                }
              })
            }
            onInput={(event) => setName(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                submit();
              }
              if (event.key === "Escape") {
                close();
              }
            }}
          />
        </label>

        <Show when={props.error()}>
          <div class="alert alert-error py-2 text-sm">
            <span class="break-all">{props.error()}</span>
          </div>
        </Show>
      </div>
    </Modal>
  );
};
