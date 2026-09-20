import type { Accessor, Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";

import { Button } from "@/components/common/daisy-ui/Button";
import { Modal } from "@/components/common/daisy-ui/Modal";
import type { DpiStrategy } from "@/types";

interface Props {
  opened: Accessor<boolean>;
  setOpened: (value: boolean) => void;
  busy: Accessor<boolean>;
  error: Accessor<string | null>;
  /** The strategy being edited; null — add mode. */
  strategy: Accessor<DpiStrategy | null>;
  onAdd: (name: string, args: string) => Promise<boolean>;
  onUpdate: (id: number, name: string, args: string) => Promise<boolean>;
  onDelete: (id: number) => Promise<boolean>;
}

export const StrategyModal: Component<Props> = (props) => {
  const [name, setName] = createSignal("");
  const [args, setArgs] = createSignal("");

  // The modal stays mounted for exit animations, so drafts reset on open —
  // prefilled from the strategy in edit mode.
  createEffect(() => {
    if (!props.opened()) {
      return;
    }
    const editing = props.strategy();
    setName(editing?.name ?? "");
    setArgs(editing?.args ?? "");
  });

  const canSubmit = () => !props.busy() && !!name().trim();

  const close = () => props.setOpened(false);

  const submit = () => {
    if (!canSubmit()) {
      return;
    }
    const editing = props.strategy();
    const request = editing
      ? props.onUpdate(editing.id, name().trim(), args().trim())
      : props.onAdd(name().trim(), args().trim());
    void request.then((ok) => {
      if (ok) {
        close();
      }
    });
  };

  const remove = () => {
    const editing = props.strategy();
    if (!editing || props.busy()) {
      return;
    }
    void props.onDelete(editing.id).then((ok) => {
      if (ok) {
        close();
      }
    });
  };

  return (
    <Modal
      opened={props.opened}
      setOpened={props.setOpened}
      title={props.strategy() ? "Edit strategy" : "Add strategy"}
      class="max-w-md"
      actions={
        <>
          <Show when={props.strategy()}>
            <Button
              variant="error"
              class="mr-auto"
              disabled={props.busy()}
              onClick={remove}
            >
              Delete
            </Button>
          </Show>
          <Button ghost disabled={props.busy()} onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!canSubmit()} onClick={submit}>
            {props.strategy() ? "Save" : "Add"}
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
            placeholder="e.g. Split SNI"
            value={name()}
            onInput={(event) => setName(event.currentTarget.value)}
          />
        </label>

        <label class="block">
          <span class="label-text mb-1 block text-sm">Arguments</span>
          <textarea
            class="textarea textarea-bordered min-h-24 w-full font-mono text-sm"
            placeholder="-s2 -d2 or --fake -1 --ttl 8 (empty = defaults)"
            value={args()}
            onInput={(event) => setArgs(event.currentTarget.value)}
          />
          <span class="label-text-alt mt-1 text-base-content/50">
            Listen address, port, daemon mode and friends are managed by the app and
            cannot be used in a strategy.
          </span>
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
