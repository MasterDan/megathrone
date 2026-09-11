/* @refresh reload */
import { render } from "solid-js/web";
import { Route, Router } from "@solidjs/router";

import { AppLayout } from "./components/layout/AppLayout";
import { Home } from "./pages/Home";
import { About } from "./pages/About";

import "./index.css";

const root = document.getElementById("root");

render(
  () => (
    <Router root={AppLayout}>
      <Route path="/" component={Home} />
      <Route path="/about" component={About} />
    </Router>
  ),
  root!,
);
