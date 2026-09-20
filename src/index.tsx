/* @refresh reload */
import { render } from "solid-js/web";
import { Route, Router } from "@solidjs/router";

import { AppLayout } from "./components/layout/AppLayout";
import { Home } from "./pages/Home";
import { Settings } from "./pages/Settings";
import { Stats } from "./pages/Stats";
import { CategoryPage } from "./pages/settings/CategoryPage";
import { ProfilesLayout } from "./pages/profiles/ProfilesLayout";
import { ProfilesList } from "./pages/profiles/ProfilesList";
import { ProfileEndpoints } from "./pages/profiles/ProfileEndpoints";

import "./index.css";

const root = document.getElementById("root");

render(
  () => (
    <Router root={AppLayout}>
      <Route path="/" component={Home} />
      <Route path="/stats" component={Stats} />
      <Route path="/profiles" component={ProfilesLayout}>
        <Route path="/" component={ProfilesList} />
        <Route path="/:id" component={ProfileEndpoints} />
      </Route>
      <Route path="/settings" component={Settings} />
      <Route path="/settings/categories/:id" component={CategoryPage} />
    </Router>
  ),
  root!,
);
