import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { A, useLocation, useNavigate } from "@solidjs/router";
import { Dynamic } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import {
  TbOutlineAlertTriangle,
  TbOutlinePlus,
  TbOutlineRadar,
  TbOutlineSettings,
  TbOutlineShieldLock,
  TbOutlineX,
} from "solid-icons/tb";

import { CrownMark } from "@/components/common/CrownMark";
import { ModeControls } from "@/components/home/ModeControls";
import { AddProfileModal } from "@/components/profiles/AddProfileModal";
import { useConnection } from "@/hooks/data/useConnection";
import { useDpiStrategies } from "@/hooks/data/useDpi";
import { useProfiles } from "@/hooks/data/useProfiles";
import { proxyMode, selectedProfileId, selectProfile, setProxyMode } from "@/stores/session";

interface NavItem {
  href: string;
  label: string;
  icon: Component<{ class?: string }>;
}

/** DPI strategies are a first-class destination on desktop (a page of its
 *  own, not a Settings tab like on mobile). */
const NAV_ITEMS: NavItem[] = [
  { href: "/dpi", label: "DPI", icon: TbOutlineShieldLock },
  { href: "/settings", label: "Settings", icon: TbOutlineSettings },
];

/**
 * The desktop control column: brand, proxy mode + connect, DPI/Settings on
 * one row, then the profile list — clicking a profile opens its endpoints
 * page, the toggle on the row makes it the session profile (the one
 * Connect uses; live-switched while connected). The heart marks the
 * connected profile; the floating plus at the bottom adds a new one.
 */
export const DesktopSidebar: Component = () => {
  const location = useLocation();
  const navigate = useNavigate();
  const { profiles, refetch } = useProfiles();
  const connection = useConnection();

  const [adding, setAdding] = createSignal(false);

  const connected = () => connection.snapshot()?.connected ?? false;
  const isActive = (href: string) => location.pathname.startsWith(href);

  // the profile the user is looking at: the endpoints page's one, or the
  // session selection on the dashboard — highlighted with a row background
  // (selection itself is the toggle's job, not the background's)
  const viewedProfileId = createMemo<number | null>(() => {
    const match = /^\/profiles\/(\d+)/.exec(location.pathname);
    if (match) {
      return Number(match[1]);
    }
    return location.pathname === "/" ? selectedProfileId() : null;
  });

  // a stale selection (the profile was deleted) falls back to the first one
  createEffect(() => {
    const list = profiles();
    if (list === undefined || list.length === 0) {
      return;
    }
    if (!list.some((profile) => profile.id === selectedProfileId())) {
      selectProfile(list[0].id);
    }
  });

  // the toggle is the session-profile switch: the live session re-points
  // while connected (same as the mobile Home dropdown, reported through the
  // restart-result toast); viewing endpoints is a plain navigation and
  // never switches anything
  const onToggleProfile = (id: number, event: Event) => {
    const input = event.currentTarget as HTMLInputElement;
    if (id === selectedProfileId()) {
      // the active profile's toggle stays on — there is always a selection
      input.checked = true;
      return;
    }
    selectProfile(id);
    if (connected() && !connection.busy()) {
      void invoke("connection_switch_profile", { profileId: id }).catch(() => {
        // the backend reports failures through the restart-result toast
      });
    }
  };

  const onConnectClick = () => {
    if (connection.busy()) {
      return;
    }
    if (connected()) {
      void connection.disconnect();
      return;
    }
    const id = selectedProfileId();
    if (id === null) {
      return;
    }
    // a rejected connect can leave a live session behind while the snapshot
    // says otherwise — re-adopt the backend truth so the crown never lies
    void connection.connect(id, proxyMode()).then(() => void connection.refresh());
  };

  // the raw local proxy address copies to the clipboard (mirrors the mobile
  // Home status line) — apps with a built-in proxy setting point here
  const [copiedPort, setCopiedPort] = createSignal(false);
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

  // DPI status under the brand (mirrors the mobile Home status line): the
  // running tunnel's strategy while connected, the marked strategy
  // otherwise (muted — not running); click opens it on the DPI page
  const dpi = useDpiStrategies();
  const dpiStatus = () => {
    const running = connection.snapshot()?.dpi;
    const list = dpi.strategies();
    if (running) {
      return {
        id: list.find((strategy) => strategy.name === running.strategy)?.id ?? null,
        name: running.strategy,
        running: true,
        title: `DPI tunnel on 127.0.0.1:${running.port} — open in DPI`,
      };
    }
    const active = list.find((strategy) => strategy.isActive);
    return active
      ? {
          id: active.id,
          name: active.name,
          running: false,
          title: "Open the active DPI strategy",
        }
      : null;
  };

  const openDpiStrategy = (id: number | null) => {
    navigate(id !== null ? `/dpi?strategy=${id}` : "/dpi");
  };

  return (
    <aside class="flex w-80 shrink-0 flex-col border-r border-base-content/10 bg-base-100">
      <div class="flex flex-col gap-3 p-4">
        {/* The brand crown IS the connect control: violet — disconnected,
            emerald — connected, spinner while the session is flipping. */}
        <div class="flex items-center justify-between">
          <button
            type="button"
            class="flex size-24 shrink-0 cursor-pointer flex-col items-center justify-center gap-1 rounded-full bg-gradient-to-br text-white shadow-lg ring-2 transition-all active:scale-95 disabled:pointer-events-none disabled:opacity-40 disabled:active:scale-100"
            classList={{
              "from-violet-500 to-violet-700 shadow-violet-700/30 ring-violet-500/20 hover:from-violet-500 hover:to-violet-600":
                !connected(),
              "from-emerald-500 to-emerald-700 shadow-emerald-700/30 ring-emerald-500/25":
                connected(),
            }}
            title={connected() ? "Disconnect" : "Connect the selected profile"}
            aria-label={connected() ? "Disconnect" : "Connect"}
            aria-pressed={connected()}
            disabled={(!connected() && selectedProfileId() === null) || connection.busy()}
            onClick={onConnectClick}
          >
            <Show
              when={!connection.busy()}
              fallback={<span class="loading loading-spinner size-7" />}
            >
              <CrownMark class="size-7" />
            </Show>
            <span class="text-xs font-semibold tracking-wide">
              {connected() ? "Disconnect" : "Connect"}
            </span>
          </button>
          <div class="flex min-w-0 flex-col items-end gap-1">
            <span class="text-lg font-bold leading-tight">Megathrone</span>
            <Show when={connection.snapshot()?.rawProxyPort}>
              {(port) => (
                <button
                  type="button"
                  class="cursor-pointer font-mono text-xs text-base-content/50 underline-offset-2 transition-colors hover:text-base-content hover:underline"
                  title="Copy 127.0.0.1:port — point apps with a built-in proxy setting here; everything they send goes through the selected endpoint"
                  onClick={copyRawProxy}
                >
                  {copiedPort() ? "copied!" : `127.0.0.1:${port()}`}
                </button>
              )}
            </Show>
            <Show when={dpiStatus()} keyed>
              {(status) => (
                <button
                  type="button"
                  class="cursor-pointer text-xs underline-offset-2 transition-colors hover:underline"
                  classList={{
                    "text-base-content/50 hover:text-base-content": !status.running,
                    "text-success hover:text-success": status.running,
                  }}
                  title={status.title}
                  onClick={() => openDpiStrategy(status.id)}
                >
                  DPI: {status.name}
                </button>
              )}
            </Show>
          </div>
        </div>

        <ModeControls
          mode={proxyMode}
          onChange={setProxyMode}
          disabled={() => connected() || connection.busy()}
        />

        {/* Connect failures surface here: the sidebar owns the connect
            control, the mobile toast stack is not mounted on desktop. */}
        <Show when={connection.error()}>
          {(message) => (
            <div class="alert alert-error items-start py-2 text-xs" role="alert">
              <TbOutlineAlertTriangle size={14} class="shrink-0" />
              <span class="min-w-0 flex-1 break-all leading-snug">{message()}</span>
              <button
                type="button"
                class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full transition-colors hover:bg-base-content/20"
                aria-label="Dismiss error"
                onClick={connection.dismissError}
              >
                <TbOutlineX size={12} />
              </button>
            </div>
          )}
        </Show>

        <nav aria-label="Primary" class="flex gap-1">
          <For each={NAV_ITEMS}>
            {(item) => (
              <A
                href={item.href}
                class="btn btn-ghost flex-1 gap-2 rounded-xl"
                classList={{
                  "bg-base-content/10 text-base-content": isActive(item.href),
                  "text-base-content/70": !isActive(item.href),
                }}
                title={item.label}
              >
                <Dynamic component={item.icon} class="size-4" />
                {item.label}
              </A>
            )}
          </For>
        </nav>
      </div>

      <div class="mx-4 border-t border-base-content/10" aria-hidden="true" />

      <div class="min-h-0 flex-1 overflow-y-auto p-4 pt-3">
        <p class="px-1 pb-2 text-[10px] font-semibold uppercase tracking-widest text-base-content/40">
          Profiles
        </p>
        <div class="flex flex-col gap-0.5">
          <For each={profiles() ?? []}>
            {(profile) => (
              <div
                class="flex items-center gap-1.5 rounded-xl py-1 pl-3 pr-2 transition-colors"
                classList={{
                  "bg-base-content/10": profile.id === viewedProfileId(),
                  "hover:bg-base-content/5": profile.id !== viewedProfileId(),
                }}
              >
                <button
                  type="button"
                  class="min-w-0 flex-1 cursor-pointer py-1.5 text-left"
                  title="Show this profile's endpoints"
                  onClick={() => navigate(`/profiles/${profile.id}`)}
                >
                  <span class="block truncate text-sm font-medium" title={profile.name}>
                    {profile.name}
                  </span>
                </button>
                <input
                  type="checkbox"
                  class="toggle toggle-primary toggle-sm shrink-0"
                  aria-label="Use this profile"
                  title={
                    profile.id === selectedProfileId()
                      ? "Selected for connection"
                      : "Use this profile"
                  }
                  checked={profile.id === selectedProfileId()}
                  onChange={(event) => onToggleProfile(profile.id, event)}
                />
              </div>
            )}
          </For>
        </div>
      </div>

      <div class="flex items-center gap-2 p-4 pt-2">
        <button
          type="button"
          class="btn btn-ghost flex-1 justify-start gap-2 rounded-xl text-base-content/70"
          onClick={() => setAdding(true)}
        >
          <TbOutlinePlus class="size-4" />
          Add profile
        </button>
        <button
          type="button"
          class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
          title="Discovery"
          aria-label="Discovery"
          onClick={() => navigate("/discovery")}
        >
          <TbOutlineRadar size={18} />
        </button>
      </div>

      <AddProfileModal opened={adding} setOpened={setAdding} onDone={() => void refetch()} />
    </aside>
  );
};
