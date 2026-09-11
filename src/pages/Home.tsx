import type { Component } from "solid-js";
import { Match, Switch } from "solid-js";
import { A } from "@solidjs/router";
import {
  TbOutlineAlertTriangle,
  TbOutlineArrowRight,
  TbOutlineRouter,
  TbOutlineShieldLock,
} from "solid-icons/tb";

import { useSingBoxVersion } from "../hooks/data/useSingBoxVersion";

export const Home: Component = () => {
  const { version } = useSingBoxVersion();

  return (
    <div class="hero bg-base-200 min-h-[calc(100vh-4rem)]">
      <div class="hero-content text-center">
        <div class="max-w-md space-y-4">
          <TbOutlineShieldLock size={72} class="mx-auto text-primary" />
          <h1 class="text-5xl font-bold">Megathrone</h1>
          <p class="py-4 text-base-content/70">
            Tauri v2 + Vite + SolidJS + Tailwind CSS + DaisyUI is up and running.
          </p>
          <Switch>
            <Match when={version.error}>
              <div class="badge badge-error badge-outline gap-1">
                <TbOutlineAlertTriangle size={14} />
                sing-box unavailable
              </div>
            </Match>
            <Match when={version.loading}>
              <span class="loading loading-spinner loading-sm text-primary" />
            </Match>
            <Match when={version()}>
              <div class="badge badge-outline gap-2 opacity-70">
                <TbOutlineRouter size={14} />
                {version()}
              </div>
            </Match>
          </Switch>
          <div>
            <A href="/about" class="btn btn-primary gap-2">
              Go to About
              <TbOutlineArrowRight size={18} />
            </A>
          </div>
        </div>
      </div>
    </div>
  );
};
