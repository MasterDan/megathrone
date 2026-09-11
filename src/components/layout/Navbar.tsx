import type { Component } from "solid-js";
import { A } from "@solidjs/router";

export const Navbar: Component = () => (
  <div class="navbar border-b border-base-300 bg-base-100">
    <div class="flex-1">
      <A href="/" class="btn btn-ghost text-xl">
        Megathrone
      </A>
    </div>
    <div class="flex-none">
      <ul class="menu menu-horizontal gap-1 px-1">
        <li>
          <A href="/" end={true} activeClass="active">
            Home
          </A>
        </li>
        <li>
          <A href="/about" activeClass="active">
            About
          </A>
        </li>
      </ul>
    </div>
  </div>
);
