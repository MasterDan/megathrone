import type { Component } from "solid-js";

import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { ProfileEndpoints } from "@/pages/profiles/ProfileEndpoints";
import { ProfilesList } from "@/pages/profiles/ProfilesList";

/** The plain profiles grid (the mobile list page) inside the content zone. */
export const DesktopProfiles: Component = () => (
  <DesktopPage>
    <ProfilesList />
  </DesktopPage>
);

/** One profile's endpoints (the mobile detail page): on desktop the page
 *  brings its own shell — the header content moves into the page header,
 *  the grid into the content zone. */
export const DesktopProfileEndpoints: Component = () => <ProfileEndpoints />;
