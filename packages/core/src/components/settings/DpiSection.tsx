import type { Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";

import { TransitionCollapse } from "@/components/common/transitions/TransitionCollapse";
import { DpiStrategiesSection } from "@/components/settings/DpiStrategiesSection";
import { useDpiSettings } from "@/hooks/data/useDpi";

interface DpiSectionProps {
  /** deep link (?strategy=<id>): scroll this strategy into view */
  revealStrategyId?: number | null;
  /** called once the reveal scroll happened (the caller cleans the URL) */
  onRevealDone?: () => void;
}

export const DpiSection: Component<DpiSectionProps> = (props) => {
  const { settings, loading, portBusy, toggleBusy, error, setPort, setEnabled } =
    useDpiSettings();
  const [draft, setDraft] = createSignal("1080");
  const [edited, setEdited] = createSignal(false);
  const [saved, setSaved] = createSignal(false);

  const dpiOn = () => settings()?.enabled ?? false;

  // adopt the backend value once loaded (and after a successful save); an
  // edited draft is never clobbered — the toggle round-trip also refreshes
  // `settings` and must not reset an unsaved port
  createEffect(() => {
    const value = settings()?.port;
    if (value === undefined || edited()) {
      return;
    }
    setDraft(String(value));
  });

  const dirty = () => Number(draft()) !== settings()?.port;
  const valid = () => {
    const parsed = Number(draft());
    return Number.isInteger(parsed) && parsed >= 1 && parsed <= 65535;
  };

  const submit = () => {
    if (portBusy() || !dirty() || !valid()) {
      return;
    }
    void setPort(Number(draft())).then((ok) => {
      if (ok) {
        setEdited(false);
        setSaved(true);
        window.setTimeout(() => setSaved(false), 1500);
      }
    });
  };

  return (
    <>
      <section class="card bg-base-content/5 shadow-sm">
        <div class="card-body gap-4 p-5">
          <div>
            <h2 class="text-lg font-bold">DPI</h2>
            <p class="mt-1 text-sm text-base-content/60">
              The master switch for the byedpi (ciadpi) tunnel and the local port it
              listens on. Applied on the next connect.
            </p>
          </div>

          <Show
            when={!loading()}
            fallback={<span class="loading loading-spinner loading-sm" />}
          >
            {/* one card-body child for both controls: the port row collapses
                out of the DOM, so its spacing lives inside the animated block
                instead of the card's gap (which would snap on unmount) */}
            <div>
              <label class="flex items-center justify-between gap-3 rounded-xl bg-base-200/60 px-3 py-2.5">
                <span class="min-w-0">
                  <span class="block text-sm font-medium">DPI bypass</span>
                  <span class="block text-xs text-base-content/50">
                    Off — connecting starts the proxy only: dpi-routed hosts fall through
                    to the routing fallback.
                  </span>
                </span>
                <input
                  type="checkbox"
                  class="toggle toggle-primary shrink-0"
                  checked={dpiOn()}
                  disabled={toggleBusy()}
                  title={
                    dpiOn()
                      ? "Disable — proxy-only sessions, the tunnel never starts"
                      : "Enable — route dpi traffic through the byedpi tunnel"
                  }
                  aria-label="DPI bypass"
                  onChange={(event) => void setEnabled(event.currentTarget.checked)}
                />
              </label>

              <TransitionCollapse>
                <Show when={dpiOn()}>
                  <div class="flex items-end gap-2 pt-4">
                    <label class="form-control w-40">
                      <div class="label py-1">
                        <span class="label-text">Listen port</span>
                      </div>
                      <input
                        type="number"
                        min="1"
                        max="65535"
                        class="input input-bordered w-full"
                        value={draft()}
                        disabled={portBusy()}
                        onInput={(event) => {
                          setEdited(true);
                          setDraft(event.currentTarget.value);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            submit();
                          }
                        }}
                      />
                    </label>
                    <button
                      type="button"
                      class="inline-flex h-8 shrink-0 cursor-pointer select-none items-center justify-center rounded-full px-3 text-sm font-medium whitespace-nowrap transition-colors disabled:pointer-events-none disabled:opacity-40"
                      classList={{
                        "bg-primary/20 text-primary hover:bg-primary/30":
                          !saved(),
                        "bg-success/20 text-success hover:bg-success/30":
                          saved(),
                      }}
                      disabled={portBusy() || !dirty() || !valid()}
                      onClick={submit}
                    >
                      <Show
                        when={!portBusy()}
                        fallback={<span class="loading loading-spinner loading-xs" />}
                      >
                        {saved() ? "Saved" : "Save"}
                      </Show>
                    </button>
                  </div>
                </Show>
              </TransitionCollapse>
            </div>

            <Show when={error()}>
              <div class="alert alert-error py-2 text-sm">
                <span class="break-all">{error()}</span>
              </div>
            </Show>
          </Show>
        </div>
      </section>

      <DpiStrategiesSection
        enabled={dpiOn}
        revealStrategyId={props.revealStrategyId}
        onRevealDone={props.onRevealDone}
      />
    </>
  );
};
