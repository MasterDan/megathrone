import type { Component } from "solid-js";

import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { Stats } from "@/pages/Stats";

/** Session statistics (the mobile page) inside the content zone. */
export const DesktopStats: Component = () => (
  <DesktopPage>
    <Stats />
  </DesktopPage>
);
