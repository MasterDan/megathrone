import type { Component } from "solid-js";
import { Show, createEffect, createSignal } from "solid-js";
import { TbOutlineClock, TbOutlineMinimize, TbOutlinePlug } from "solid-icons/tb";

import { useGeneralSettings } from "@/hooks/data/useGeneralSettings";

const MINUTES_MAX = 1440;
const PERCENT_MAX = 100;
const PORT_MAX = 65535;

/**
 * Settings section (General): the automatic-strategy settings — how often
 * round robin rotates to the next endpoint, how often the reachable ones
 * are re-tested, and the availability floor below which an endpoint counts
 * as dead — plus the close-to-tray behavior of the window's close button
 * and the raw local proxy port.
 */
export const GeneralSection: Component = () => {
  const { settings, loading, busy, error, saveCadences, saveCloseToTray, saveRawProxy } =
    useGeneralSettings();

  const [rotation, setRotation] = createSignal("10");
  const [recheck, setRecheck] = createSignal("10");
  const [minAvailability, setMinAvailability] = createSignal("55");
  const [savedCard, setSavedCard] = createSignal<string | null>(null);
  const [rawPortDraft, setRawPortDraft] = createSignal("7890");
  const [rawPortEdited, setRawPortEdited] = createSignal(false);

  // adopt the backend values once loaded (and after a successful save) —
  // but never over a card that holds unsaved edits, so saving one card
  // does not revert the other one mid-edit
  createEffect(() => {
    const current = settings();
    if (!current) {
      return;
    }
    if (!cadencesDirty()) {
      setRotation(String(current.rotationMinutes));
      setRecheck(String(current.recheckMinutes));
      setMinAvailability(String(current.minAvailabilityPercent));
    }
  });

  const parsed = () => ({
    rotation: Number(rotation()),
    recheck: Number(recheck()),
    minAvailability: Number(minAvailability()),
  });
  /** Per-card dirty flags — one card's edits never arm the other's Save. */
  const cadencesDirty = () =>
    !!settings() &&
    (parsed().rotation !== settings()!.rotationMinutes ||
      parsed().recheck !== settings()!.recheckMinutes ||
      parsed().minAvailability !== settings()!.minAvailabilityPercent);
  /** Per-card validity — a save only touches its own card's values. */
  const cadencesValid = () => {
    const values = parsed();
    return (
      Number.isInteger(values.rotation) &&
      values.rotation >= 1 &&
      values.rotation <= MINUTES_MAX &&
      Number.isInteger(values.recheck) &&
      values.recheck >= 1 &&
      values.recheck <= MINUTES_MAX &&
      Number.isInteger(values.minAvailability) &&
      values.minAvailability >= 0 &&
      values.minAvailability <= PERCENT_MAX
    );
  };

  const submit = () => {
    if (busy() || !cadencesValid()) {
      return;
    }
    void saveCadences(
      parsed().rotation,
      parsed().recheck,
      parsed().minAvailability,
    ).then((ok) => {
      if (ok) {
        setSavedCard("cadences");
        window.setTimeout(() => setSavedCard(null), 1500);
      }
    });
  };

  // adopt the raw port once loaded (and after a successful save); an
  // edited draft is never clobbered — the toggle round-trip also refreshes
  // `settings` and must not reset an unsaved port
  createEffect(() => {
    const value = settings()?.rawProxyPort;
    if (value === undefined || rawPortEdited()) {
      return;
    }
    setRawPortDraft(String(value));
  });

  const rawPortDirty = () =>
    !!settings() && Number(rawPortDraft()) !== settings()!.rawProxyPort;
  const rawPortValid = () => {
    const port = Number(rawPortDraft());
    return Number.isInteger(port) && port >= 1 && port <= PORT_MAX;
  };

  const submitRawPort = () => {
    if (busy() || !rawPortValid()) {
      return;
    }
    void saveRawProxy(settings()!.rawProxyEnabled, Number(rawPortDraft())).then((ok) => {
      if (ok) {
        setRawPortEdited(false);
        setSavedCard("rawPort");
        window.setTimeout(() => setSavedCard(null), 1500);
      }
    });
  };

  const toggleRawProxy = (enabled: boolean) => {
    if (busy() || !settings()) {
      return;
    }
    void saveRawProxy(enabled, settings()!.rawProxyPort).then((ok) => {
      if (ok) {
        setSavedCard("rawProxy");
        window.setTimeout(() => setSavedCard(null), 1500);
      }
    });
  };

  /** Shared numeric-input row of the automatic-strategies card. */
  const CadenceInput: Component<{
    label: string;
    hint: string;
    value: () => string;
    onInput: (value: string) => void;
    ariaLabel: string;
    min?: number;
    max?: number;
  }> = (props) => (
    <label class="w-44">
      <div class="py-1">
        <span class="text-sm">{props.label}</span>
      </div>
      <input
        type="number"
        min={props.min ?? 1}
        max={props.max ?? MINUTES_MAX}
        class="input input-bordered w-full"
        value={props.value()}
        disabled={busy()}
        aria-label={props.ariaLabel}
        onInput={(event) => props.onInput(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            submit();
          }
        }}
      />
      <span class="mt-1 block text-xs text-base-content/50">{props.hint}</span>
    </label>
  );

  return (
    <section class="flex flex-col gap-4">
      <div class="card bg-base-content/5 shadow-sm">
        <div class="card-body gap-4 p-5">
          <div>
            <h2 class="flex items-center gap-2 text-lg font-bold">
              <TbOutlineClock size={20} class="text-base-content/50" />
              Automatic strategies
            </h2>
            <p class="mt-1 text-sm text-base-content/60">
              While a profile with an automatic strategy is connected, its endpoints are
              kept healthy in the background: reachable ones are re-tested to refresh
              their ping and availability, and round robin rotates to the next living
              endpoint on its own schedule. Endpoints whose site checks fall below the
              minimum availability are treated as dead and never picked.
            </p>
          </div>

          <Show
            when={!loading()}
            fallback={<span class="loading loading-spinner loading-sm" />}
          >
            <div class="flex flex-wrap items-start gap-4">
              <CadenceInput
                label="Rotate endpoints"
                hint="minutes between switches (round robin)"
                value={rotation}
                onInput={setRotation}
                ariaLabel="Rotation interval in minutes"
              />
              <CadenceInput
                label="Re-test reachable"
                hint="minutes between background re-checks"
                value={recheck}
                onInput={setRecheck}
                ariaLabel="Re-check interval in minutes"
              />
              <CadenceInput
                label="Min availability"
                hint="% of site checks an endpoint must pass (0 — off)"
                value={minAvailability}
                onInput={setMinAvailability}
                ariaLabel="Minimum availability in percent"
                min={0}
                max={PERCENT_MAX}
              />
              {/* mirrors a field column's label line + input box, so the
                  button centers on the inputs no matter how the hints wrap */}
              <div class="flex flex-col">
                <div class="py-1">
                  <span class="invisible text-sm" aria-hidden="true">
                    .
                  </span>
                </div>
                <div class="flex h-10 items-center">
                  <button
                    type="button"
                    class="inline-flex h-8 shrink-0 cursor-pointer select-none items-center justify-center rounded-full px-3 text-sm font-medium whitespace-nowrap transition-colors disabled:pointer-events-none disabled:opacity-40"
                    classList={{
                      "bg-primary/20 text-primary hover:bg-primary/30":
                        savedCard() !== "cadences",
                      "bg-success/20 text-success hover:bg-success/30":
                        savedCard() === "cadences",
                    }}
                    disabled={busy() || !cadencesValid()}
                    onClick={() => submit()}
                  >
                    <Show
                      when={!busy()}
                      fallback={<span class="loading loading-spinner loading-xs" />}
                    >
                      {savedCard() === "cadences" ? "Saved" : "Save"}
                    </Show>
                  </button>
                </div>
              </div>
            </div>
          </Show>
        </div>
      </div>

      <div class="card bg-base-content/5 shadow-sm">
        <div class="card-body gap-4 p-5">
          <div>
            <h2 class="flex items-center gap-2 text-lg font-bold">
              <TbOutlineMinimize size={20} class="text-base-content/50" />
              Close behavior
            </h2>
            <p class="mt-1 text-sm text-base-content/60">
              What the window close button does. Hidden to the tray, Megathrone
              keeps running — the proxy session, profile auto-updates and
              endpoint supervision continue until you quit it from the tray
              menu.
            </p>
          </div>

          <Show
            when={!loading()}
            fallback={<span class="loading loading-spinner loading-sm" />}
          >
            <div class="flex items-center justify-between gap-4">
              <div>
                <span class="text-sm font-medium">Close to tray</span>
                <span class="mt-1 block text-xs text-base-content/50">
                  {(settings()?.closeToTray ?? true)
                    ? "The close button hides Megathrone to the tray icon"
                    : "The close button quits Megathrone entirely"}
                </span>
              </div>
              <div class="flex items-center gap-3">
                <Show when={savedCard() === "close"}>
                  <span class="text-xs text-success">Saved</span>
                </Show>
                <input
                  type="checkbox"
                  class="toggle toggle-primary"
                  checked={settings()?.closeToTray ?? true}
                  disabled={busy()}
                  aria-label="Close to tray"
                  onChange={(event) => {
                    void saveCloseToTray(event.currentTarget.checked).then((ok) => {
                      if (ok) {
                        setSavedCard("close");
                        window.setTimeout(() => setSavedCard(null), 1500);
                      }
                    });
                  }}
                />
              </div>
            </div>
          </Show>
        </div>
      </div>

      <div class="card bg-base-content/5 shadow-sm">
        <div class="card-body gap-4 p-5">
          <div>
            <h2 class="flex items-center gap-2 text-lg font-bold">
              <TbOutlinePlug size={20} class="text-base-content/50" />
              Local proxy
            </h2>
            <p class="mt-1 text-sm text-base-content/60">
              An extra port for apps with a built-in proxy setting: point them at
              127.0.0.1:<span class="font-mono">{settings()?.rawProxyPort ?? 7890}</span> and
              everything they send — private ranges included — goes through the
              currently selected endpoint, ignoring the routing rules. The port
              follows the main session's endpoint switches and exists only while
              the proxy is connected.
            </p>
          </div>

          <Show
            when={!loading()}
            fallback={<span class="loading loading-spinner loading-sm" />}
          >
            <div class="flex flex-wrap items-start gap-4">
              <label class="flex cursor-pointer flex-col">
                <div class="py-1">
                  <span class="text-sm">Enabled</span>
                </div>
                <div class="flex h-10 items-center">
                  <input
                    type="checkbox"
                    class="toggle toggle-primary"
                    checked={settings()?.rawProxyEnabled ?? true}
                    disabled={busy()}
                    aria-label="Local proxy"
                    onChange={(event) => toggleRawProxy(event.currentTarget.checked)}
                  />
                </div>
                <span class="mt-1 block text-xs text-base-content/50">
                  {(settings()?.rawProxyEnabled ?? true)
                    ? "The port opens with the next connect"
                    : "No extra port is opened"}
                </span>
              </label>

              <label class="w-40">
                <div class="py-1">
                  <span class="text-sm">Port</span>
                </div>
                <input
                  type="number"
                  min="1"
                  max={PORT_MAX}
                  class="input input-bordered w-full"
                  value={rawPortDraft()}
                  disabled={busy()}
                  aria-label="Local proxy port"
                  onInput={(event) => {
                    setRawPortEdited(true);
                    setRawPortDraft(event.currentTarget.value);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      submitRawPort();
                    }
                  }}
                />
                <span class="mt-1 block text-xs text-base-content/50">
                  falls back to a random free port when taken
                </span>
              </label>

              <div class="flex flex-col">
                <div class="py-1">
                  <span class="invisible text-sm" aria-hidden="true">
                    .
                  </span>
                </div>
                <div class="flex h-10 items-center">
                  <Show
                    when={savedCard() === "rawProxy"}
                    fallback={
                      <button
                        type="button"
                        class="inline-flex h-8 shrink-0 cursor-pointer select-none items-center justify-center rounded-full px-3 text-sm font-medium whitespace-nowrap transition-colors disabled:pointer-events-none disabled:opacity-40"
                        classList={{
                          "bg-primary/20 text-primary hover:bg-primary/30":
                            savedCard() !== "rawPort",
                          "bg-success/20 text-success hover:bg-success/30":
                            savedCard() === "rawPort",
                        }}
                        disabled={busy() || !rawPortDirty() || !rawPortValid()}
                        onClick={() => submitRawPort()}
                      >
                        <Show
                          when={!busy()}
                          fallback={<span class="loading loading-spinner loading-xs" />}
                        >
                          {savedCard() === "rawPort" ? "Saved" : "Save"}
                        </Show>
                      </button>
                    }
                  >
                    <span class="text-xs text-success">Saved</span>
                  </Show>
                </div>
              </div>
            </div>
          </Show>
        </div>
      </div>

      <Show when={error()}>
        <div class="alert alert-error py-2 text-sm">
          <span class="break-all">{error()}</span>
        </div>
      </Show>
    </section>
  );
};
