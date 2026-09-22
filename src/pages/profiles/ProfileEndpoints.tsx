import type { Accessor, Component } from "solid-js";
import type { RouteSectionProps } from "@solidjs/router";
import { Match, Show, Switch, createMemo, createSignal } from "solid-js";
import { A, useLocation, useNavigate, useParams } from "@solidjs/router";
import {
  TbOutlineActivity,
  TbOutlineArrowLeft,
  TbOutlineClockCog,
  TbOutlineFoldersOff,
  TbOutlineInfoCircle,
  TbOutlinePencil,
  TbOutlinePlayerStop,
  TbOutlineRefresh,
  TbOutlineX,
} from "solid-icons/tb";

import { Empty } from "@/components/common/Empty";
import { OptionTabs, type OptionTabItem } from "@/components/common/daisy-ui/OptionTabs";
import { Modal } from "@/components/common/daisy-ui/Modal";
import { TrafficDock } from "@/components/home/TrafficDock";
import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { useIsMobileUi } from "@/contexts/uiVariant";
import { EditProfileModal } from "@/components/profiles/EditProfileModal";
import { ProfileItems, type ProfileItemsApi } from "@/components/profiles/ProfileItems";
import { PROTOCOL_BADGES } from "@/components/profiles/EndpointCard";
import { useConnection } from "@/hooks/data/useConnection";
import { useLatencyCheck } from "@/hooks/data/useLatencyCheck";
import { useProfile } from "@/hooks/data/useProfile";
import { useProfileActions } from "@/hooks/data/useProfileActions";
import { useProfileSelection } from "@/hooks/data/useProfileSelection";
import { useProfileUpdating } from "@/hooks/data/useProfileUpdating";
import { SELECTION_MODE_LABELS } from "@/types";
import type { EndpointItem, ProfileSummary, SelectionMode } from "@/types";
import { formatInterval, formatRelativeTime } from "@/utils/time";

/** The strategy tabs: how the profile's live endpoint is chosen. */
const SELECTION_TABS: Array<OptionTabItem<SelectionMode>> = (
  ["round_robin", "fastest", "most_available", "manual"] as const
).map((value) => ({ value, label: SELECTION_MODE_LABELS[value] }));

function sourceLabel(profile: { sourceUrl: string | null; sourcePath: string | null }) {
  if (profile.sourceUrl) {
    try {
      return new URL(profile.sourceUrl).host;
    } catch {
      return profile.sourceUrl;
    }
  }
  if (profile.sourcePath) {
    const parts = profile.sourcePath.split(/[/\\]/);
    return parts[parts.length - 1] || profile.sourcePath;
  }
  return "pasted text";
}

/** Header row 1: identity, the edit pencil next to the name, the live
 *  selection chip; the back button is mobile-only (the desktop sidebar
 *  navigates). Shared by the mobile sticky panel and the desktop page
 *  header. */
const HeaderIdentity: Component<{
  profile: ProfileSummary;
  selected: Accessor<EndpointItem | null | undefined>;
  mode: Accessor<SelectionMode>;
  onReveal: (itemId: number) => void;
  onClearSelection: () => void;
  onEdit: () => void;
  showBack?: boolean;
}> = (props) => (
  <>
    <Show when={props.showBack !== false}>
      <A
        href="/profiles"
        class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
        title="Back to profiles"
        aria-label="Back to profiles"
      >
        <TbOutlineArrowLeft size={16} />
      </A>
    </Show>
    <div class="min-w-0">
      <h1 class="truncate text-lg font-bold leading-tight" title={props.profile.name}>
        {props.profile.name}
      </h1>
      <p
        class="truncate text-xs text-base-content/50"
        title={props.profile.sourceUrl ?? props.profile.sourcePath ?? undefined}
      >
        {sourceLabel(props.profile)}
      </p>
    </div>
    <button
      type="button"
      class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
      title="Edit profile"
      aria-label="Edit profile"
      onClick={props.onEdit}
    >
      <TbOutlinePencil size={16} />
    </button>
    <div class="ml-auto flex min-w-0 items-center gap-1.5">
      <Show
        when={props.selected()}
        fallback={
          <span class="truncate text-xs text-base-content/40">
            {props.mode() === "manual" ? "Pick an endpoint below" : "Choosing automatically…"}
          </span>
        }
      >
        {(item) => (
          <>
            {/* The chip is also a jump link: clicking it scrolls the grid
                to the selected card. */}
            <button
              type="button"
              class="flex min-w-0 items-center gap-1.5 rounded-full bg-base-content/5 px-1.5 py-0.5 transition-colors hover:bg-base-content/10"
              title="Show this endpoint in the list"
              aria-label="Show this endpoint in the list"
              onClick={() => props.onReveal(item().id)}
            >
              <span
                class={`badge badge-sm shrink-0 ${
                  PROTOCOL_BADGES[item().protocol] ?? "badge-ghost"
                }`}
              >
                {item().protocol}
              </span>
              <span class="truncate text-xs font-medium" title={item().tag}>
                {item().tag}
              </span>
              <Show when={item().latencyMs != null}>
                <span class="whitespace-nowrap text-xs font-medium text-success">
                  {item().latencyMs} ms
                </span>
              </Show>
            </button>
            <Show when={props.mode() === "manual"}>
              <button
                type="button"
                class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
                title="Unpick the endpoint"
                aria-label="Unpick the endpoint"
                onClick={props.onClearSelection}
              >
                <TbOutlineX size={12} />
              </button>
            </Show>
          </>
        )}
      </Show>
    </div>
  </>
);

/** Header row 2: the selection strategy tabs + profile stats. */
const HeaderTabs: Component<{
  profile: ProfileSummary;
  mode: Accessor<SelectionMode>;
  onModeChange: (mode: SelectionMode) => void;
  onStats: () => void;
}> = (props) => (
  <>
    <OptionTabs
      class="min-w-0 flex-1"
      items={SELECTION_TABS}
      value={props.mode()}
      onChange={props.onModeChange}
    />
    {/* The stats sit inline on wide screens; below `lg` they collapse into
        the info modal. */}
    <div class="hidden shrink-0 items-center gap-1.5 whitespace-nowrap text-xs text-base-content/50 lg:flex">
      <span title="Endpoints in this profile">{props.profile.itemCount} endpoints</span>
      <Show when={props.profile.skippedCount > 0}>
        <span aria-hidden="true">·</span>
        <span title="Lines that could not be parsed">{props.profile.skippedCount} skipped</span>
      </Show>
      <Show when={props.profile.autoUpdateMinutes}>
        {(minutes) => (
          <>
            <span aria-hidden="true">·</span>
            <span class="inline-flex items-center gap-1" title="Auto-update interval">
              <TbOutlineClockCog size={12} />
              auto {formatInterval(minutes())}
            </span>
          </>
        )}
      </Show>
      <span aria-hidden="true">·</span>
      <span title={`Fetched ${formatRelativeTime(props.profile.lastFetchedAt)}`}>
        updated {formatRelativeTime(props.profile.lastFetchedAt)}
      </span>
    </div>
    <button
      type="button"
      class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content lg:hidden"
      title="Profile stats"
      aria-label="Profile stats"
      onClick={props.onStats}
    >
      <TbOutlineInfoCircle size={16} />
    </button>
  </>
);

/** Desktop footer: the live traffic charts slide in only while connected
 *  (the mobile dock's behavior); clicking opens the session statistics
 *  page. */
const ChartsFooter: Component = () => {
  const navigate = useNavigate();
  const connection = useConnection();
  const connected = () => connection.snapshot()?.connected ?? false;
  const openStats = () => navigate("/stats");
  return (
    // the wrapper's height animates so nothing jumps; the inner block
    // keeps its size while clipped
    <div
      classList={{
        "overflow-hidden transition-[height] duration-500 ease-in-out": true,
        "h-0": !connected(),
        "h-44": connected(),
      }}
    >
      <div
        role="link"
        tabIndex={0}
        classList={{
          "h-44 cursor-pointer p-3": true,
          "border-t border-base-content/10": connected(),
        }}
        title="Open session statistics"
        onClick={openStats}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            openStats();
          }
        }}
      >
        <TrafficDock />
      </div>
    </div>
  );
};

// Partial<RouteSectionProps>: the component renders both as a route
// section (router injects params/location) and standalone (the desktop
// dashboard passes just the profile id).
export const ProfileEndpoints: Component<Partial<RouteSectionProps> & { profileId?: number }> = (
  props,
) => {
  const params = useParams();
  const location = useLocation();
  const navigate = useNavigate();
  // an explicit id (the desktop dashboard renders the session-selected
  // profile) wins over the /profiles/:id route param
  const profileId = createMemo(() => props.profileId ?? Number(params.id));
  const isMobile = useIsMobileUi();
  const { profile, loading, error } = useProfile(profileId);
  const selection = useProfileSelection(profileId);

  const { busy: manualUpdate, update } = useProfileActions(() => {
    // refresh arrives via the profiles-changed event
  });
  const { updating: remotelyUpdating } = useProfileUpdating(profileId);

  // true while any update of this profile runs — clicked or scheduled
  const updating = () => manualUpdate() || remotelyUpdating();

  const {
    checking: checkingLatency,
    stopping: stoppingLatency,
    progress: latencyProgress,
    error: latencyError,
    check: checkLatency,
    cancel: cancelLatency,
  } = useLatencyCheck(profileId);

  const [editing, setEditing] = createSignal(false);
  const [statsOpen, setStatsOpen] = createSignal(false);

  /** Imperative API of the endpoints grid (reveal-an-endpoint), handed
   *  over by ProfileItems on mount. */
  let itemsApi: ProfileItemsApi | undefined;

  // Deep link from Home (?reveal=<itemId>): scroll the grid to that
  // endpoint as soon as the grid hands over its API, then clean the URL.
  const requestedReveal = Number(new URLSearchParams(location.search).get("reveal"));
  let pendingReveal: number | null = Number.isInteger(requestedReveal) ? requestedReveal : null;

  const runPendingReveal = () => {
    if (pendingReveal === null) {
      return;
    }
    const itemId = pendingReveal;
    pendingReveal = null;
    navigate(`/profiles/${profileId()}`, { replace: true });
    itemsApi?.revealItem(itemId);
  };

  const hasSource = (data: { sourceUrl: string | null; sourcePath: string | null }) =>
    Boolean(data.sourceUrl || data.sourcePath);

  const activeMode = (): SelectionMode => selection.mode() ?? "fastest";

  const progressPercent = () => {
    const current = latencyProgress();
    return current && current.total > 0 ? (current.done / current.total) * 100 : 0;
  };

  const BackAction: Component = () => (
    <A
      href="/profiles"
      class="inline-flex h-8 cursor-pointer items-center justify-center gap-1 rounded-full bg-base-content/10 px-3 text-sm font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
    >
      <TbOutlineArrowLeft size={16} />
      Back to profiles
    </A>
  );

  /** The grid + per-profile modals — the same body under both layouts. */
  const profileBody = (data: Accessor<ProfileSummary>) => (
    <>
      <Show when={latencyError()}>
        <div class="alert alert-error py-2 text-sm">
          <span class="break-all">{latencyError()}</span>
        </div>
      </Show>

      <ProfileItems
        profileId={data().id}
        total={data().itemCount}
        selectedId={selection.selected()?.id ?? null}
        exposeApi={(api) => {
          itemsApi = api;
          runPendingReveal();
        }}
        onSelect={(item) => {
          // re-picking the running endpoint would needlessly flip the
          // mode to manual and restart the session
          if (item.id !== selection.selected()?.id) {
            void selection.select(item);
          }
        }}
      />

      <EditProfileModal
        profile={data()}
        opened={editing}
        setOpened={setEditing}
        onDone={() => setEditing(false)}
      />

      {/* Narrow screens only (the ⓘ button): the stats that sit inline in
          the header on wide screens. */}
      <Modal
        title="Profile stats"
        opened={statsOpen}
        setOpened={setStatsOpen}
        class="max-w-sm"
      >
        <div class="space-y-2 text-sm">
          <div class="flex items-baseline justify-between gap-4">
            <span class="text-base-content/50">Endpoints</span>
            <span class="font-medium">{data().itemCount}</span>
          </div>
          <Show when={data().skippedCount > 0}>
            <div class="flex items-baseline justify-between gap-4">
              <span class="text-base-content/50">Skipped lines</span>
              <span class="font-medium">{data().skippedCount}</span>
            </div>
          </Show>
          <div class="flex items-baseline justify-between gap-4">
            <span class="text-base-content/50">Auto-update</span>
            <Show
              when={data().autoUpdateMinutes}
              fallback={<span class="font-medium">Off</span>}
            >
              {(minutes) => (
                <span class="font-medium">every {formatInterval(minutes())}</span>
              )}
            </Show>
          </div>
          <div class="flex items-baseline justify-between gap-4">
            <span class="text-base-content/50">Last fetched</span>
            <span class="font-medium">{formatRelativeTime(data().lastFetchedAt)}</span>
          </div>
          <div class="flex items-baseline justify-between gap-4">
            <span class="shrink-0 text-base-content/50">Source</span>
            <span class="break-all text-right text-xs text-base-content/70">
              {data().sourceUrl ?? data().sourcePath ?? "pasted text"}
            </span>
          </div>
        </div>
      </Modal>
    </>
  );

  return (
    <Switch>
      {/* Desktop: the header content lives in the page header (above the
          content zone) as the same two rows the in-page dock had — row 1:
          identity + selection chip + edit, row 2: strategy tabs + stats.
          The zone holds the grid; the Update/Check-Latency actions live in
          the status bar. */}
      <Match when={!isMobile()}>
        <DesktopPage
          title={
            <Show when={profile()}>
              {(data) => (
                <div class="flex w-full min-w-0 flex-col gap-2">
                  <div class="flex w-full items-center gap-2">
                    <HeaderIdentity
                      profile={data()}
                      showBack={false}
                      selected={selection.selected}
                      mode={activeMode}
                      onReveal={(itemId) => itemsApi?.revealItem(itemId)}
                      onClearSelection={() => void selection.clear()}
                      onEdit={() => setEditing(true)}
                    />
                  </div>
                  <div class="flex w-full items-center gap-2">
                    <HeaderTabs
                      profile={data()}
                      mode={activeMode}
                      onModeChange={(mode) => void selection.setMode(mode)}
                      onStats={() => setStatsOpen(true)}
                    />
                  </div>
                </div>
              )}
            </Show>
          }
          footer={<ChartsFooter />}
        >
          <div class="space-y-6">
            <Show when={loading()}>
              <div class="flex justify-center py-16">
                <span class="loading loading-spinner text-primary" />
              </div>
            </Show>

            <Show when={error()}>
              <Empty
                icon={TbOutlineFoldersOff}
                title="Profile not found"
                description="It may have been deleted."
                action={BackAction}
              />
            </Show>

            <Show when={profile()}>{(data) => profileBody(data)}</Show>
          </div>
        </DesktopPage>
      </Match>

      {/* Mobile: the header is a sticky floating panel inside the page and
          the Update/Check-Latency dock floats over the window bottom. */}
      <Match when={isMobile()}>
        <div class="space-y-6 pb-28">
          <Show when={loading()}>
            <div class="flex justify-center py-16">
              <span class="loading loading-spinner text-primary" />
            </div>
          </Show>

          <Show when={error()}>
            <Empty
              icon={TbOutlineFoldersOff}
              title="Profile not found"
              description="It may have been deleted."
              action={BackAction}
            />
          </Show>

          <Show when={profile()}>
            {(data) => (
              <>
                <div class="sticky top-14 z-40 -mx-4 rounded-2xl border border-base-content/10 bg-base-100/60 px-4 py-2.5 shadow-sm backdrop-blur-md">
                  {/* Row 1: identity — live selection. */}
                  <div class="flex items-center gap-2">
                    <HeaderIdentity
                      profile={data()}
                      selected={selection.selected}
                      mode={activeMode}
                      onReveal={(itemId) => itemsApi?.revealItem(itemId)}
                      onClearSelection={() => void selection.clear()}
                      onEdit={() => setEditing(true)}
                    />
                  </div>

                  {/* Row 2: strategy — profile stats. */}
                  <div class="mt-2 flex items-center gap-2">
                    <HeaderTabs
                      profile={data()}
                      mode={activeMode}
                      onModeChange={(mode) => void selection.setMode(mode)}
                      onStats={() => setStatsOpen(true)}
                    />
                  </div>
                </div>

                {profileBody(data)}

                {/* The Update/Check Latency dock: desktop renders these
                    actions in the status bar instead. */}
                <div class="pointer-events-none fixed inset-x-0 bottom-0 z-50 flex justify-center px-4 pb-3">
                    <div
                      class="pointer-events-auto relative flex items-center gap-1 rounded-2xl border border-base-content/10 bg-base-100/60 p-1 shadow-lg backdrop-blur-md transition-[padding] duration-200"
                      classList={{ "pb-2.5": !checkingLatency(), "pb-6": checkingLatency() }}
                    >
                      <button
                        type="button"
                        class="inline-flex h-8 cursor-pointer select-none items-center justify-center gap-1.5 rounded-full bg-base-content/10 px-3 text-sm font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
                        title={
                          updating()
                            ? "Updating…"
                            : hasSource(data())
                              ? "Update now"
                              : "No source to update from"
                        }
                        disabled={!hasSource(data()) || updating()}
                        onClick={() => void update(data().id)}
                      >
                        <Show
                          when={!updating()}
                          fallback={<span class="loading loading-spinner loading-xs" />}
                        >
                          <TbOutlineRefresh size={16} />
                        </Show>
                        Update
                      </button>
                      <button
                        type="button"
                        class="inline-flex h-8 cursor-pointer select-none items-center justify-center gap-1.5 rounded-full px-3 text-sm font-medium whitespace-nowrap transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary disabled:pointer-events-none disabled:opacity-40"
                        classList={{
                          "bg-base-content/10 text-base-content hover:bg-base-content/20":
                            !checkingLatency(),
                          "bg-primary/20 text-primary hover:bg-primary/30":
                            checkingLatency(),
                        }}
                        title={
                          checkingLatency()
                            ? "Stop the running scan"
                            : "URL-test every endpoint through sing-box"
                        }
                        disabled={stoppingLatency()}
                        onClick={() =>
                          checkingLatency() ? void cancelLatency() : void checkLatency()
                        }
                      >
                        <Switch>
                          <Match when={stoppingLatency()}>
                            <span class="loading loading-spinner loading-xs" />
                            Stopping…
                          </Match>
                          <Match when={checkingLatency()}>
                            <TbOutlinePlayerStop size={16} />
                            Stop Testing
                          </Match>
                          <Match when={true}>
                            <TbOutlineActivity size={16} />
                            Check Latency
                          </Match>
                        </Switch>
                      </button>
                      <Show when={checkingLatency()}>
                        <div class="absolute inset-x-3 bottom-[5px] flex items-center gap-2">
                          <div
                            class="h-[3px] flex-1 overflow-hidden rounded-full bg-base-content/10"
                            role="progressbar"
                            aria-label="Latency test progress"
                            aria-valuemin={0}
                            aria-valuemax={100}
                            aria-valuenow={Math.round(progressPercent())}
                          >
                            <div
                              class="h-full rounded-full bg-primary transition-[width] duration-300"
                              style={{ width: `${progressPercent()}%` }}
                            />
                          </div>
                          <Show when={latencyProgress()}>
                            {(progress) => (
                              <span class="whitespace-nowrap text-[10px] leading-none text-base-content/50 tabular-nums">
                                {progress().done} / {progress().total}
                              </span>
                            )}
                          </Show>
                        </div>
                      </Show>
                    </div>
                  </div>
              </>
            )}
          </Show>
        </div>
      </Match>
    </Switch>
  );
};
