import type { Component } from "solid-js";
import { For, Match, Show, Switch, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { Dynamic } from "solid-js/web";
import { useLocation } from "@solidjs/router";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  TbOutlineActivity,
  TbOutlineInfoCircle,
  TbOutlinePlayerStop,
  TbOutlineRefresh,
  TbOutlineShieldLock,
} from "solid-icons/tb";

import { AboutModal } from "@/components/layout/AboutModal";
import { useProfileActions } from "@/hooks/data/useProfileActions";
import { useProfiles } from "@/hooks/data/useProfiles";
import { selectedProfileId } from "@/stores/session";
import {
  DPI_TEST_FINISHED_EVENT,
  DPI_TEST_PROGRESS_EVENT,
  LATENCY_PROGRESS_EVENT,
  PROFILE_UPDATE_EVENT,
  PROFILES_CHANGED_EVENT,
} from "@/types";
import type {
  DpiTestProgress,
  LatencyProgress,
  ProfileUpdateNotice,
  ProfilesChangedPayload,
} from "@/types";

interface ScanActivity {
  done: number;
  total: number;
}

interface UpdateActivity {
  name: string;
}

/** Compact progress chip for background activity outside the viewed
 *  profile: another profile's scan, the DPI strategy test, an update. */
const ProgressChip: Component<{
  label: string;
  done: number;
  total: number;
  icon: Component<{ class?: string }>;
  onStop?: () => void;
}> = (props) => {
  const percent = () => (props.total > 0 ? (props.done / props.total) * 100 : 0);
  return (
    <span class="flex shrink-0 items-center gap-2 rounded-full bg-base-content/5 py-1 pl-2.5 pr-1.5">
      <Dynamic component={props.icon} class="size-3.5 shrink-0 text-base-content/50" />
      <span class="max-w-40 truncate" title={props.label}>
        {props.label}
      </span>
      <span
        class="h-1 w-16 overflow-hidden rounded-full bg-base-content/10"
        role="progressbar"
        aria-label={props.label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(percent())}
      >
        <span
          class="block h-full rounded-full bg-primary transition-[width] duration-300"
          style={{ width: `${percent()}%` }}
        />
      </span>
      <span class="whitespace-nowrap tabular-nums text-base-content/50">
        {props.done}/{props.total}
      </span>
      <Show when={props.onStop}>
        {(stop) => (
          <button
            type="button"
            class="flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-full text-base-content/40 transition-colors hover:bg-base-content/10 hover:text-base-content"
            title="Stop"
            aria-label={`Stop ${props.label}`}
            onClick={stop()}
          >
            <TbOutlinePlayerStop size={12} />
          </button>
        )}
      </Show>
    </span>
  );
};

/** The viewed profile's scan — stretched across the free bar width. */
const CurrentScanBar: Component<{ name: string; progress: ScanActivity | null }> = (props) => {
  const percent = () =>
    props.progress && props.progress.total > 0
      ? (props.progress.done / props.progress.total) * 100
      : 0;
  return (
    <div class="flex min-w-0 flex-1 items-center gap-2">
      <TbOutlineActivity class="size-3.5 shrink-0 text-base-content/50" />
      <span class="max-w-48 shrink truncate" title={`Testing ${props.name}`}>
        Testing {props.name}
      </span>
      <div
        class="h-1 min-w-8 flex-1 overflow-hidden rounded-full bg-base-content/10"
        role="progressbar"
        aria-label={`Testing ${props.name}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(percent())}
      >
        <span
          class="block h-full rounded-full bg-primary transition-[width] duration-300"
          style={{ width: `${percent()}%` }}
        />
      </div>
      <Show
        when={props.progress}
        fallback={<span class="text-base-content/40" aria-label="Starting">…</span>}
      >
        {(progress) => (
          <span class="whitespace-nowrap tabular-nums text-base-content/50">
            {progress().done}/{progress().total}
          </span>
        )}
      </Show>
    </div>
  );
};

/**
 * The desktop status line across the window bottom: the Update / Check
 * Latency actions for the profile in view (the /profiles/:id one, or the
 * session selection elsewhere — the same profile whose page would carry
 * the mobile action dock), the stretched live progress of its scan, and
 * compact chips for everything else running in the background.
 */
export const TestStatusBar: Component = () => {
  const location = useLocation();
  const { profiles } = useProfiles();
  const actions = useProfileActions(() => {});

  const [aboutOpen, setAboutOpen] = createSignal(false);
  const [scans, setScans] = createSignal<Record<number, ScanActivity>>({});
  const [dpiTest, setDpiTest] = createSignal<ScanActivity | null>(null);
  const [updates, setUpdates] = createSignal<Record<number, UpdateActivity>>({});
  // clicked Check Latency before the first progress batch arrived
  const [pendingScan, setPendingScan] = createSignal<number | null>(null);
  const [stoppingScan, setStoppingScan] = createSignal(false);

  // the profile whose actions this bar carries: an endpoints page's profile,
  // or the session selection on every other page
  const currentProfileId = createMemo<number | null>(() => {
    const match = /^\/profiles\/(\d+)/.exec(location.pathname);
    return match ? Number(match[1]) : selectedProfileId();
  });

  // pages that display a profile's endpoints: there the bar carries the
  // Update/Check-Latency actions and the stretched scan progress; anywhere
  // else every scan shows as a compact chip only
  const endpointContext = createMemo(
    () => location.pathname === "/" || /^\/profiles\/\d+/.test(location.pathname),
  );

  const currentProfile = () =>
    profiles()?.find((profile) => profile.id === currentProfileId());
  const currentScan = () => {
    const id = currentProfileId();
    return id !== null ? (scans()[id] ?? null) : null;
  };
  const scanningCurrent = () => currentPending() || currentScan() !== null;
  const currentPending = () => {
    const id = currentProfileId();
    return id !== null && pendingScan() === id;
  };
  const currentUpdating = () => {
    const id = currentProfileId();
    return id !== null && (actions.busy() || id in updates());
  };
  const currentHasSource = () => {
    const profile = currentProfile();
    return Boolean(profile && (profile.sourceUrl || profile.sourcePath));
  };

  const scanEntries = () => Object.entries(scans());
  const backgroundScanEntries = () =>
    endpointContext()
      ? scanEntries().filter(([id]) => Number(id) !== currentProfileId())
      : scanEntries();
  const updateEntries = () => Object.entries(updates());
  const currentName = () => {
    const profile = currentProfile();
    return profile?.name ?? `Profile #${currentProfileId()}`;
  };

  const checkLatency = async () => {
    const id = currentProfileId();
    if (id === null || scanningCurrent()) {
      return;
    }
    setPendingScan(id);
    try {
      await invoke("profile_check_latency", { profileId: id });
    } catch {
      // failures surface through the page's error alert / backend events
    } finally {
      setPendingScan(null);
    }
  };

  const stopLatency = async () => {
    const id = currentProfileId();
    if (id === null || stoppingScan()) {
      return;
    }
    setStoppingScan(true);
    try {
      await invoke("profile_cancel_latency", { profileId: id });
    } catch {
      // the scan keeps running; the button releases itself
    } finally {
      setStoppingScan(false);
    }
  };

  const cancelBackgroundScan = (profileId: number) => {
    void invoke("profile_cancel_latency", { profileId }).catch(() => {});
  };
  const cancelDpiTest = () => {
    void invoke("dpi_cancel_test").catch(() => {});
  };

  onMount(() => {
    let disposed = false;
    const keep = (promise: Promise<() => void>) => {
      void promise.then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          onCleanup(() => unlisten());
        }
      });
    };

    // adopt a strategy test that is already running
    void invoke<{ done: number; total: number } | null>("dpi_test_status").then((status) => {
      if (!disposed && status) {
        setDpiTest({ done: status.done, total: status.total });
      }
    });

    keep(
      listen<LatencyProgress>(LATENCY_PROGRESS_EVENT, (event) => {
        const { profileId, done, total } = event.payload;
        setScans((current) => ({ ...current, [profileId]: { done, total } }));
      }),
    );

    // the scan is over (finished, cancelled or failed) — drop its entry
    keep(
      listen<ProfilesChangedPayload>(PROFILES_CHANGED_EVENT, (event) => {
        const profileId = event.payload.profileId;
        if (event.payload.kind !== "latency" || profileId === null) {
          return;
        }
        setScans((current) => {
          if (!(profileId in current)) {
            return current;
          }
          const next = { ...current };
          delete next[profileId];
          return next;
        });
      }),
    );

    keep(
      listen<DpiTestProgress>(DPI_TEST_PROGRESS_EVENT, (event) => {
        setDpiTest({ done: event.payload.done, total: event.payload.total });
      }),
    );
    keep(listen(DPI_TEST_FINISHED_EVENT, () => setDpiTest(null)));

    keep(
      listen<ProfileUpdateNotice>(PROFILE_UPDATE_EVENT, (event) => {
        const notice = event.payload;
        setUpdates((current) => {
          if (notice.kind === "started") {
            return { ...current, [notice.profileId]: { name: notice.profileName } };
          }
          if (!(notice.profileId in current)) {
            return current;
          }
          const next = { ...current };
          delete next[notice.profileId];
          return next;
        });
      }),
    );

    onCleanup(() => {
      disposed = true;
    });
  });

  return (
    <>
      <footer
        class="flex h-10 shrink-0 items-center gap-3 overflow-hidden border-t border-base-content/10 bg-base-100 px-3 text-xs"
        aria-label="Test progress"
      >
        {/* About lives in the bar's left corner — deliberately small and
            dim: an occasional lookup, not an action */}
        <button
          type="button"
          class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full text-base-content/35 transition-colors hover:bg-base-content/10 hover:text-base-content/70"
          title="About"
          aria-label="About"
          onClick={() => setAboutOpen(true)}
        >
          <TbOutlineInfoCircle size={14} />
        </button>

        <div class="flex min-w-0 flex-1 items-center">
        <Show when={endpointContext() && scanningCurrent()}>
          <CurrentScanBar name={currentName()} progress={currentScan()} />
        </Show>
      </div>

      <div class="flex shrink-0 items-center gap-2">
        <For each={updateEntries()}>
          {([, update]) => (
            <span class="flex shrink-0 items-center gap-1.5 rounded-full bg-base-content/5 px-2.5 py-1">
              <span class="loading loading-spinner size-3 shrink-0" />
              <span class="max-w-40 truncate" title={`Updating ${update.name}`}>
                Updating {update.name}
              </span>
            </span>
          )}
        </For>
        <For each={backgroundScanEntries()}>
          {([id, scan]) => (
            <ProgressChip
              label={`Testing ${profileName(id, profiles)}`}
              done={scan.done}
              total={scan.total}
              icon={TbOutlineActivity}
              onStop={() => cancelBackgroundScan(Number(id))}
            />
          )}
        </For>
        <Show when={dpiTest()}>
          {(progress) => (
            <ProgressChip
              label="DPI test"
              done={progress().done}
              total={progress().total}
              icon={TbOutlineShieldLock}
              onStop={cancelDpiTest}
            />
          )}
        </Show>
      </div>

      <Show when={endpointContext() && currentProfileId() !== null}>
        <div class="flex shrink-0 items-center gap-1">
          <button
            type="button"
            class="btn btn-ghost btn-xs gap-1.5 rounded-lg"
            title={
              currentUpdating()
                ? "Updating…"
                : currentHasSource()
                  ? "Update now"
                  : "No source to update from"
            }
            disabled={!currentHasSource() || currentUpdating()}
            onClick={() => {
              const id = currentProfileId();
              if (id !== null) {
                void actions.update(id);
              }
            }}
          >
            <Show
              when={!currentUpdating()}
              fallback={<span class="loading loading-spinner loading-xs" />}
            >
              <TbOutlineRefresh size={14} />
            </Show>
            Update
          </button>
          <button
            type="button"
            class="btn btn-xs gap-1.5 rounded-lg border-0"
            classList={{
              "bg-base-content/10 text-base-content hover:bg-base-content/20":
                !scanningCurrent(),
              "bg-primary/20 text-primary hover:bg-primary/30": scanningCurrent(),
            }}
            title={
              scanningCurrent()
                ? "Stop the running scan"
                : "URL-test every endpoint through sing-box"
            }
            disabled={stoppingScan()}
            onClick={() => void (scanningCurrent() ? stopLatency() : checkLatency())}
          >
            <Switch>
              <Match when={stoppingScan()}>
                <span class="loading loading-spinner loading-xs" />
                Stopping…
              </Match>
              <Match when={scanningCurrent()}>
                <TbOutlinePlayerStop size={14} />
                Stop Testing
              </Match>
              <Match when={true}>
                <TbOutlineActivity size={14} />
                Check Latency
              </Match>
            </Switch>
          </button>
        </div>
      </Show>
      </footer>

      <AboutModal opened={aboutOpen} setOpened={setAboutOpen} />
    </>
  );
};

const profileName = (id: string, profiles: () => Array<{ id: number; name: string }> | undefined) =>
  profiles()?.find((profile) => profile.id === Number(id))?.name ?? `Profile #${id}`;
