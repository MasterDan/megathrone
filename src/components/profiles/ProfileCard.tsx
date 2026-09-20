import type { Component } from "solid-js";
import { Show } from "solid-js";
import { useNavigate } from "@solidjs/router";
import { TbOutlineClockCog, TbOutlinePencil } from "solid-icons/tb";

import { formatInterval, formatRelativeTime } from "@/utils/time";
import type { ProfileSummary } from "@/types";

interface Props {
  profile: ProfileSummary;
  onEdit: () => void;
}

function sourceLabel(profile: ProfileSummary) {
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

export const ProfileCard: Component<Props> = (props) => {
  const navigate = useNavigate();

  return (
    <div
      class="card cursor-pointer bg-base-content/5 shadow-sm transition-shadow hover:shadow-md"
      role="link"
      tabIndex={0}
      title="Open endpoints"
      onClick={() => navigate(`/profiles/${props.profile.id}`)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          navigate(`/profiles/${props.profile.id}`);
        }
      }}
    >
      <div class="card-body gap-3 p-4">
        <div class="flex items-start justify-between gap-2">
          <div class="min-w-0">
            <h3 class="truncate text-lg font-semibold" title={props.profile.name}>
              {props.profile.name}
            </h3>
            <p
              class="truncate text-xs text-base-content/50"
              title={props.profile.sourceUrl ?? props.profile.sourcePath ?? undefined}
            >
              {sourceLabel(props.profile)}
            </p>
          </div>
          <button
            type="button"
            class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
            title="Edit profile"
            onClick={(event) => {
              event.stopPropagation();
              props.onEdit();
            }}
          >
            <TbOutlinePencil size={16} />
          </button>
        </div>

        <div class="flex flex-wrap items-center gap-2 text-xs">
          <span class="badge badge-ghost">{props.profile.itemCount} endpoints</span>
          <Show when={props.profile.skippedCount > 0}>
            <span class="badge badge-ghost opacity-60" title="Lines that could not be parsed">
              {props.profile.skippedCount} skipped
            </span>
          </Show>
          <Show when={props.profile.autoUpdateMinutes}>
            {(minutes) => (
              <span class="badge badge-primary badge-outline" title="Auto-update interval">
                <TbOutlineClockCog size={12} />
                auto {formatInterval(minutes())}
              </span>
            )}
          </Show>
          <span class="opacity-40" title={`Fetched ${formatRelativeTime(props.profile.lastFetchedAt)}`}>
            {formatRelativeTime(props.profile.lastFetchedAt)}
          </span>
        </div>
      </div>
    </div>
  );
};
