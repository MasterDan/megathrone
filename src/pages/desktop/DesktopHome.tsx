import type { Component } from "solid-js";
import { Show } from "solid-js";
import { TbOutlineFoldersOff } from "solid-icons/tb";

import { Empty } from "@/components/common/Empty";
import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { ProfileEndpoints } from "@/pages/profiles/ProfileEndpoints";
import { selectedProfileId } from "@/stores/session";

/**
 * The desktop dashboard: endpoints of the session-selected profile (the
 * sidebar toggle feeds it). The shell — header, grid zone and the
 * connected-only traffic charts footer — belongs to ProfileEndpoints
 * itself.
 */
export const DesktopHome: Component = () => (
  <Show
    when={selectedProfileId()}
    keyed
    fallback={
      <DesktopPage title={<h1 class="text-lg font-bold">Megathrone</h1>}>
        <Empty
          icon={TbOutlineFoldersOff}
          title="No profile selected"
          description="Pick a profile in the sidebar to see its endpoints."
        />
      </DesktopPage>
    }
  >
    {(id) => <ProfileEndpoints profileId={id} />}
  </Show>
);
