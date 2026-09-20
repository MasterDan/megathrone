import type { Component } from "solid-js";
import type { RouteSectionProps } from "@solidjs/router";

export const ProfilesLayout: Component<RouteSectionProps> = (props) => (
  <div class="container mx-auto px-4 pb-4 pt-2">{props.children}</div>
);
