import type { RouteSectionProps } from "@solidjs/router";
import type { ParentComponent } from "solid-js";

import { NotificationsToast } from "@/components/layout/NotificationsToast";
import { TrafficStatsProvider } from "@/hooks/data/useTrafficStats";
import { UrlStatsProvider } from "@/hooks/data/useUrlStats";

import { DesktopSidebar } from "./DesktopSidebar";
import { TestStatusBar } from "./TestStatusBar";

/**
 * The desktop router root: the control column on the left, the routed page
 * (own header + rounded content zone) on the right, the test-progress
 * status line across the bottom. The traffic providers live above the
 * routes so the charts' rolling window keeps filling while the user is on
 * another page (mirrors the mobile AppLayout).
 */
export const DesktopAppLayout: ParentComponent<RouteSectionProps> = (props) => (
  <div class="flex h-screen flex-col overflow-hidden bg-base-100">
    <div class="flex min-h-0 flex-1">
      <DesktopSidebar />
      <TrafficStatsProvider>
        <UrlStatsProvider>
          <main class="min-w-0 flex-1">{props.children}</main>
        </UrlStatsProvider>
      </TrafficStatsProvider>
    </div>
    <TestStatusBar />
    <NotificationsToast />
  </div>
);
