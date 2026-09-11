import type { Component } from "solid-js";
import { A } from "@solidjs/router";
import { TbOutlineInfoCircle } from "solid-icons/tb";

export const About: Component = () => (
  <div class="flex min-h-[calc(100vh-4rem)] items-center justify-center bg-base-200 p-4">
    <div class="card w-96 bg-base-100 shadow-xl">
      <div class="card-body gap-4">
        <h2 class="card-title gap-2">
          <TbOutlineInfoCircle size={20} />
          About
        </h2>
        <p class="text-base-content/70">An empty starter. Replace this page with your own.</p>
        <div class="card-actions justify-end">
          <A href="/" class="btn btn-primary">
            Back Home
          </A>
        </div>
      </div>
    </div>
  </div>
);
