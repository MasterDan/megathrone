import type { Component } from "solid-js";
import type { RouteSectionProps } from "@solidjs/router";

import { Navbar } from "./Navbar";

export const AppLayout: Component<RouteSectionProps> = (props) => (
  <div class="flex min-h-screen flex-col bg-base-100">
    <Navbar />
    <main class="flex-1">{props.children}</main>
  </div>
);
