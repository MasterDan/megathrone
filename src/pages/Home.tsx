import type { Component } from "solid-js";
import { A } from "@solidjs/router";

export const Home: Component = () => (
  <div class="hero bg-base-200 min-h-[calc(100vh-4rem)]">
    <div class="hero-content text-center">
      <div class="max-w-md space-y-4">
        <h1 class="text-5xl font-bold">Megathrone</h1>
        <p class="py-4 text-base-content/70">
          Tauri v2 + Vite + SolidJS + Tailwind CSS + DaisyUI is up and running.
        </p>
        <A href="/about" class="btn btn-primary">
          Go to About
        </A>
      </div>
    </div>
  </div>
);
