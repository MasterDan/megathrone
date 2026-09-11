import type { Component } from "solid-js";
import { A } from "@solidjs/router";
import { TbOutlineCrown, TbOutlineHome2, TbOutlineInfoCircle } from "solid-icons/tb";

export const Navbar: Component = () => (
  <div class="navbar border-b border-base-300 bg-base-100">
    <div class="flex-1">
      <A href="/" class="btn btn-ghost gap-2 text-xl">
        <TbOutlineCrown size={26} />
        Megathrone
      </A>
    </div>
    <div class="flex-none">
      <ul class="menu menu-horizontal gap-1 px-1">
        <li>
          <A href="/" end={true} activeClass="active">
            <TbOutlineHome2 size={18} />
            Home
          </A>
        </li>
        <li>
          <A href="/about" activeClass="active">
            <TbOutlineInfoCircle size={18} />
            About
          </A>
        </li>
      </ul>
    </div>
  </div>
);
