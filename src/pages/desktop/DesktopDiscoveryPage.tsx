import type { Component } from "solid-js";

import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { DiscoveryPage } from "@/pages/discovery/DiscoveryPage";

/** The Discovery catalog (the mobile page) inside the content zone. */
export const DesktopDiscoveryPage: Component = () => (
  <DesktopPage>
    <DiscoveryPage />
  </DesktopPage>
);
