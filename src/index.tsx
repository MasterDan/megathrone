/* @refresh reload */
import type { Component } from "solid-js";
import type { RouteSectionProps } from "@solidjs/router";
import { render } from "solid-js/web";
import { Route, Router } from "@solidjs/router";

import { AppLayout } from "./components/layout/AppLayout";
import { DesktopAppLayout } from "@/components/layout/desktop/DesktopAppLayout";
import { UiVariantProvider } from "@/contexts/uiVariant";
import { BUILD_UI_VARIANT } from "@/platform";
import { Home } from "./pages/Home";
import { Settings } from "./pages/Settings";
import { Stats } from "./pages/Stats";
import { CategoryPage } from "./pages/settings/CategoryPage";
import { ProfilesLayout } from "./pages/profiles/ProfilesLayout";
import { ProfilesList } from "./pages/profiles/ProfilesList";
import { ProfileEndpoints } from "./pages/profiles/ProfileEndpoints";
import { DesktopHome } from "@/pages/desktop/DesktopHome";
import { DesktopCategoryPage } from "@/pages/desktop/DesktopCategoryPage";
import { DesktopDpi } from "@/pages/desktop/DesktopDpi";
import { DesktopDiscoveryPage } from "@/pages/desktop/DesktopDiscoveryPage";
import { DesktopProfileEndpoints, DesktopProfiles } from "@/pages/desktop/DesktopProfiles";
import { DesktopSettings } from "@/pages/desktop/DesktopSettings";
import { DesktopStats } from "@/pages/desktop/DesktopStats";
import { DiscoveryPage } from "@/pages/discovery/DiscoveryPage";

import "./index.css";

// The build variant picks the router root and the page set — the strongly
// different layouts stay build-time decisions (see platform.ts); both trees
// share the same paths so deep links keep working in either build.
const MOBILE_UI = BUILD_UI_VARIANT === "mobile";

/** Desktop pass-through for the /profiles branch: its pages bring their own
 *  rounded content zones, the padded mobile wrapper would double-pad them. */
const BareLayout: Component<RouteSectionProps> = (props) => props.children;

const RootLayout = MOBILE_UI ? AppLayout : DesktopAppLayout;

const mountPoint = document.getElementById("root");

render(
  () => (
    <UiVariantProvider>
      <Router root={RootLayout}>
        <Route path="/" component={MOBILE_UI ? Home : DesktopHome} />
        <Route path="/stats" component={MOBILE_UI ? Stats : DesktopStats} />
        <Route path="/profiles" component={MOBILE_UI ? ProfilesLayout : BareLayout}>
          <Route path="/" component={MOBILE_UI ? ProfilesList : DesktopProfiles} />
          <Route path="/:id" component={MOBILE_UI ? ProfileEndpoints : DesktopProfileEndpoints} />
        </Route>
        <Route path="/settings" component={MOBILE_UI ? Settings : DesktopSettings} />
        <Route
          path="/settings/categories/:id"
          component={MOBILE_UI ? CategoryPage : DesktopCategoryPage}
        />
        <Route
          path="/discovery"
          component={MOBILE_UI ? DiscoveryPage : DesktopDiscoveryPage}
        />
        <Route path="/dpi" component={DesktopDpi} />
      </Router>
    </UiVariantProvider>
  ),
  mountPoint!,
);
