import type { Accessor, Component } from "solid-js";
import { Match, Switch } from "solid-js";
import { A } from "@solidjs/router";
import { TbOutlineAlertTriangle, TbOutlineArrowRight, TbOutlinePlus } from "solid-icons/tb";

import { ComboBox } from "@/components/common/daisy-ui/forms/ComboBox";
import type { ProfileSummary } from "@/types";

export const ProfileSelect: Component<{
  profiles: Accessor<ProfileSummary[] | undefined>;
  loading: Accessor<boolean>;
  error: Accessor<unknown>;
  selectedId: Accessor<number | null>;
  onSelect: (id: number) => void;
}> = (props) => {
  const list = () => props.profiles();
  const hasList = () => (list()?.length ?? 0) > 0;
  const selected = () =>
    (list() ?? []).find((profile) => profile.id === props.selectedId());

  return (
    <Switch
      fallback={
        <A
          href="/profiles"
          class="inline-flex h-8 cursor-pointer items-center justify-center gap-1 rounded-full bg-base-content/10 px-3 text-sm font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
        >
          <TbOutlinePlus size={16} />
          Add a profile
        </A>
      }
    >
      <Match when={props.error() && !list()}>
        <div class="badge badge-error badge-outline gap-1">
          <TbOutlineAlertTriangle size={14} />
          Failed to load profiles
        </div>
      </Match>
      <Match when={hasList() && list()}>
        {(profiles) => (
          <div class="flex w-72 max-w-full items-center gap-1">
            <div class="min-w-0 flex-1">
              <ComboBox
                search={(query) => {
                  const needle = query.trim().toLowerCase();
                  return profiles().filter((profile) =>
                    profile.name.toLowerCase().includes(needle),
                  );
                }}
                getKey={(profile) => profile.id}
                displayValue={(profile) => profile.name}
                placeholder="Select profile…"
                value={selected()}
                onChange={(profile) => {
                  if (profile) {
                    props.onSelect(profile.id);
                  }
                }}
              >
                {(item, api) => (
                  <button
                    type="button"
                    class="w-full rounded-lg px-3 py-2 text-left"
                    classList={{ "bg-base-200": api.selected() }}
                    onClick={api.select}
                  >
                    {item.name}
                  </button>
                )}
              </ComboBox>
            </div>
            <A
              href={
                props.selectedId() === null
                  ? "/profiles"
                  : `/profiles/${props.selectedId()}`
              }
              class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
              title={
                props.selectedId() === null
                  ? "Profiles"
                  : `Open ${selected()?.name ?? "profile"}`
              }
              aria-label="Open selected profile"
            >
              <TbOutlineArrowRight size={16} />
            </A>
          </div>
        )}
      </Match>
      <Match when={props.loading()}>
        <div class="skeleton h-10 w-72 rounded-btn" />
      </Match>
    </Switch>
  );
};
