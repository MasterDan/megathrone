import type { Component } from "solid-js";
import { Match, Show, Switch, createEffect, createSignal } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { invoke } from "@tauri-apps/api/core";
import { TbOutlineAlertTriangle, TbOutlineX } from "solid-icons/tb";

import { ConnectButton } from "@/components/home/ConnectButton";
import { ModeControls } from "@/components/home/ModeControls";
import { ProfileSelect } from "@/components/home/ProfileSelect";
import { TrafficDock } from "@/components/home/TrafficDock";
import { useConnection } from "@/hooks/data/useConnection";
import { useDpiStrategies } from "@/hooks/data/useDpi";
import { useProfiles } from "@/hooks/data/useProfiles";
import { useProfileSelection } from "@/hooks/data/useProfileSelection";
import { SELECTION_MODE_LABELS } from "@/types";
import type { ProxyMode } from "@/types";
import { BUILD_UI_VARIANT } from "@/platform";

const SELECTED_PROFILE_KEY = "megathrone.selectedProfileId";
const SELECTED_MODE_KEY = "megathrone.proxyMode";

const readStoredProfileId = () => {
  const stored = localStorage.getItem(SELECTED_PROFILE_KEY);
  const parsed = stored === null ? Number.NaN : Number(stored);
  return Number.isFinite(parsed) ? parsed : null;
};

const readStoredMode = (): ProxyMode => {
  const stored = localStorage.getItem(SELECTED_MODE_KEY);
  if (stored !== "system-proxy" && stored !== "tun") return "off";
  // a TUN selection carried over from the desktop variant cannot run on
  // mobile — degrade to the nearest supported mode instead of failing
  return stored === "tun" && BUILD_UI_VARIANT === "mobile" ? "system-proxy" : stored;
};

export const Home: Component = () => {
  const navigate = useNavigate();
  const { profiles, loading, error } = useProfiles();
  const dpi = useDpiStrategies();

  const [selectedId, setSelectedId] = createSignal<number | null>(readStoredProfileId());
  const [mode, setMode] = createSignal<ProxyMode>(readStoredMode());
  const [copiedPort, setCopiedPort] = createSignal(false);

  // which endpoint runs and how it is chosen — automatic strategies pick
  // one on connect and keep it fed in the background, so a missing
  // checkmark never blocks the connect
  const selection = useProfileSelection(() => selectedId());
  const connection = useConnection();

  const connected = () => connection.snapshot()?.connected ?? false;
  const activeMode = () => selection.mode() ?? "fastest";

  createEffect(() => {
    const id = selectedId();
    if (id === null) {
      localStorage.removeItem(SELECTED_PROFILE_KEY);
    } else {
      localStorage.setItem(SELECTED_PROFILE_KEY, String(id));
    }
  });

  // survive navigation away and back (the page unmounts on route change)
  createEffect(() => {
    localStorage.setItem(SELECTED_MODE_KEY, mode());
  });

  createEffect(() => {
    const list = profiles();
    if (list === undefined || list.length === 0) {
      return;
    }
    if (!list.some((profile) => profile.id === selectedId())) {
      setSelectedId(list[0].id);
    }
  });

  const onConnectClick = () => {
    if (connection.busy()) {
      return;
    }
    if (connected()) {
      void connection.disconnect();
      return;
    }
    const id = selectedId();
    if (id === null) {
      return;
    }
    void connection.connect(id, mode());
  };

  // while connected, picking another profile re-points the live session at
  // it (the same mode, the new profile's strategy) — the outcome arrives
  // via the restart-result toast
  const onProfileSelect = (id: number) => {
    setSelectedId(id);
    if (connected() && !connection.busy()) {
      void invoke("connection_switch_profile", { profileId: id }).catch(() => {
        // the backend reports failures through restart-result already
      });
    }
  };

  // the status line's endpoint tag is a jump link: it opens the profile
  // page scrolled to the endpoint card (the reveal param is consumed there)
  const openEndpointInProfile = () => {
    const id = selectedId();
    if (id === null) {
      return;
    }
    const item = selection.selected();
    navigate(item ? `/profiles/${id}?reveal=${item.id}` : `/profiles/${id}`);
  };

  // DPI status is always visible: the running tunnel's strategy while
  // connected, the marked strategy otherwise (muted — not running)
  const dpiStatus = () => {
    const running = connection.snapshot()?.dpi;
    const list = dpi.strategies();
    if (running) {
      return {
        id: list.find((strategy) => strategy.name === running.strategy)?.id ?? null,
        name: running.strategy,
        running: true,
        title: `DPI tunnel on 127.0.0.1:${running.port} — open in Settings`,
      };
    }
    const active = list.find((strategy) => strategy.isActive);
    return active
      ? {
          id: active.id,
          name: active.name,
          running: false,
          title: "Open the active DPI strategy in Settings",
        }
      : null;
  };

  // the raw local proxy address copies to the clipboard — apps with a
  // built-in proxy setting point here to send everything through the
  // selected endpoint
  const copyRawProxy = () => {
    const port = connection.snapshot()?.rawProxyPort;
    if (port === null || port === undefined) {
      return;
    }
    void navigator.clipboard.writeText(`127.0.0.1:${port}`).then(
      () => {
        setCopiedPort(true);
        window.setTimeout(() => setCopiedPort(false), 1500);
      },
      // clipboard may be unavailable (no focus / permission) — nothing to do
      () => {},
    );
  };

  const openDpiStrategy = (id: number | null) => {
    navigate(id !== null ? `/settings?tab=dpi&strategy=${id}` : "/settings?tab=dpi");
  };

  // does the status line render anything before the DPI part (separator)?
  const hasStatusText = () => connection.busy() || connected() || selectedId() !== null;

  return (
    // fill exactly the visible viewport (main's pt-14 = 3.5rem) so the docks
    // sit flush at the window bottom; min-h (not h) lets a too-short window
    // grow the page instead of clipping — the content then scrolls and rests
    // on the button block. The dock swaps hide off-screen via transforms,
    // which still count as document overflow — AppLayout clips those at the
    // document edge so they never produce a phantom scrollbar here.
    <div class="flex min-h-[calc(100vh-3.5rem)] w-full flex-col items-center justify-between gap-6 px-4 pb-6 pt-8">
      <div class="flex w-full max-w-2xl items-center justify-center gap-2">
        <ProfileSelect
          profiles={profiles}
          loading={loading}
          error={error}
          selectedId={selectedId}
          onSelect={onProfileSelect}
        />
      </div>

      <div class="flex flex-col items-center gap-6">
        <ConnectButton
          connected={connected}
          connecting={connection.busy}
          disabled={() => selectedId() === null}
          onClick={onConnectClick}
        />
        <div class="min-h-6 text-center text-sm text-base-content/60">
          <Switch>
            <Match when={connection.busy() && !connected()}>Connecting…</Match>
            <Match when={connected()}>
              <Show
                when={connection.snapshot()?.endpointTag}
                fallback={<>Connected — DPI only</>}
              >
                {(tag) => (
                  <>
                    Connected via{" "}
                    <button
                      type="button"
                      class="cursor-pointer underline-offset-2 hover:underline"
                      title="Show this endpoint in the profile"
                      onClick={openEndpointInProfile}
                    >
                      {tag()}
                    </button>
                  </>
                )}
              </Show>
              <Show when={connection.snapshot()?.rawProxyPort}>
                {(port) => (
                  <>
                    {" · "}
                    <button
                      type="button"
                      class="cursor-pointer font-mono underline-offset-2 hover:underline"
                      title="Copy 127.0.0.1:port — point apps with a built-in proxy setting here; everything they send goes through the selected endpoint"
                      onClick={copyRawProxy}
                    >
                      {copiedPort() ? "copied!" : `127.0.0.1:${port()}`}
                    </button>
                  </>
                )}
              </Show>
            </Match>
            <Match when={activeMode() !== "manual" && selection.selected()}>
              {(endpoint) => (
                <>
                  {SELECTION_MODE_LABELS[activeMode()]} ·{" "}
                  <button
                    type="button"
                    class="cursor-pointer underline-offset-2 hover:underline"
                    title="Show this endpoint in the profile"
                    onClick={openEndpointInProfile}
                  >
                    {endpoint().tag}
                  </button>
                </>
              )}
            </Match>
            <Match when={activeMode() !== "manual"}>
              {SELECTION_MODE_LABELS[activeMode()]} — an endpoint is picked on connect
            </Match>
            <Match when={selection.selected()}>
              {(endpoint) => (
                <>
                  via{" "}
                  <button
                    type="button"
                    class="cursor-pointer underline-offset-2 hover:underline"
                    title="Show this endpoint in the profile"
                    onClick={openEndpointInProfile}
                  >
                    {endpoint().tag}
                  </button>
                </>
              )}
            </Match>
            <Match when={selectedId() !== null}>An endpoint is picked automatically on connect</Match>
          </Switch>
          <Show when={dpiStatus()} keyed>
            {(status) => (
              <>
                <Show when={hasStatusText()}>{" · "}</Show>
                <button
                  type="button"
                  class="cursor-pointer underline-offset-2 hover:underline"
                  classList={{ "text-success": status.running }}
                  title={status.title}
                  onClick={() => openDpiStrategy(status.id)}
                >
                  DPI: {status.name}
                </button>
              </>
            )}
          </Show>
        </div>
      </div>

      {/* the bottom dock swap: connecting sends the mode dock sliding down
          and floats the live-traffic dock up in its place (and back on
          disconnect) — the wrapper's height animates so nothing jumps */}
      <div
        classList={{
          "relative w-full transition-[height] duration-500 ease-in-out": true,
          "h-12": !connected(),
          "h-72": connected(),
        }}
      >
        <div
          classList={{
            "absolute inset-x-0 bottom-0 flex justify-center transition-all duration-500 ease-in-out":
              true,
            "translate-y-0 opacity-100": !connected(),
            "pointer-events-none translate-y-[220%] opacity-0": connected(),
          }}
        >
          <ModeControls
            mode={mode}
            onChange={setMode}
            disabled={() => connected() || connection.busy()}
          />
        </div>
        {/* the live dock opens the session statistics page — the per-URL
            request summary plus these same charts */}
        <div
          role="link"
          tabIndex={0}
          classList={{
            "absolute inset-0 flex justify-center transition-all duration-500 ease-in-out":
              true,
            "pointer-events-none translate-y-[110%] opacity-0": !connected(),
            "translate-y-0 opacity-100 cursor-pointer": connected(),
          }}
          title="Open session statistics"
          onClick={() => navigate("/stats")}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              navigate("/stats");
            }
          }}
        >
          <TrafficDock />
        </div>
      </div>

      <Show when={connection.error()}>
        {(message) => (
          <div class="toast toast-center toast-bottom z-50 pb-6">
            <div class="alert alert-error items-start py-3 text-sm shadow-lg">
              <TbOutlineAlertTriangle size={18} class="shrink-0" />
              <span class="max-w-md whitespace-pre-wrap break-words">{message()}</span>
              <button
                type="button"
                class="flex size-6 cursor-pointer items-center justify-center rounded-full bg-base-content/10 transition-colors hover:bg-base-content/20"
                aria-label="Dismiss error"
                onClick={connection.dismissError}
              >
                <TbOutlineX size={14} />
              </button>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
};
