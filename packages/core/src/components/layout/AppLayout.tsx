import type { Component } from "solid-js";
import type { RouteSectionProps } from "@solidjs/router";
import { A, useLocation } from "@solidjs/router";
import { Show, createSignal } from "solid-js";
import { Dynamic } from "solid-js/web";
import { TbOutlineFolders, TbOutlineInfoCircle, TbOutlineSettings } from "solid-icons/tb";

import { AboutModal } from "@/components/layout/AboutModal";
import { NotificationsToast } from "@/components/layout/NotificationsToast";
import { CrownMark } from "@/components/common/CrownMark";
import { ScrollTopButton } from "@/components/common/ScrollTopButton";
import { TrafficStatsProvider } from "@/hooks/data/useTrafficStats";
import { UrlStatsProvider } from "@/hooks/data/useUrlStats";

interface DockTabProps {
  href: string;
  label: string;
  active: boolean;
  primary?: boolean;
  icon?: Component<{ class?: string }>;
}

const DockTab: Component<DockTabProps> = (props) => (
  <A
    href={props.href}
    aria-current={props.active ? "page" : undefined}
    class="btn btn-ghost btn-sm gap-2 rounded-xl"
    classList={{
      "text-base-content/60": !props.primary,
      "font-semibold": props.primary,
      "bg-base-content/10 text-base-content": props.active && !props.primary,
      "bg-primary/10 text-primary": props.primary && !props.active,
      "bg-primary text-primary-content hover:bg-primary hover:text-primary-content":
        props.primary && props.active,
    }}
  >
    <Show when={props.icon}>{(icon) => <Dynamic component={icon()} class="size-4" />}</Show>
    {props.label}
  </A>
);

export const AppLayout: Component<RouteSectionProps> = (props) => {
  const location = useLocation();
  const isHome = () => location.pathname === "/";
  const isProfiles = () => location.pathname.startsWith("/profiles");
  const isSettings = () => location.pathname.startsWith("/settings");
  const [aboutOpen, setAboutOpen] = createSignal(false);

  return (
    // overflow-clip: the home dock swap hides docks by translating them past
    // the window bottom — transformed boxes still count toward the document's
    // scrollable overflow, so without the clip the idle home page always
    // shows a phantom scrollbar. The shell wraps all in-flow content, so the
    // clip line is the document edge: nothing real is ever cut, and the
    // too-short-window fallback (content overflowing into a scrollbar) still
    // works — the shell grows with the content there.
    <div class="flex min-h-screen flex-col overflow-clip bg-base-100">
      <header class="pointer-events-none fixed inset-x-0 top-0 z-50 flex justify-center px-4 pt-2">
        <nav
          aria-label="Primary"
          class="pointer-events-auto flex items-center gap-1 rounded-2xl border border-base-content/10 bg-base-100/60 p-1 shadow-lg backdrop-blur-md"
        >
          <DockTab href="/" label="Megathrone" active={isHome()} primary icon={CrownMark} />
          <span class="mx-1 h-5 w-px bg-base-content/10" aria-hidden="true" />
          <DockTab href="/profiles" label="Profiles" active={isProfiles()} icon={TbOutlineFolders} />
          <DockTab href="/settings" label="Settings" active={isSettings()} icon={TbOutlineSettings} />
          <span class="mx-1 h-5 w-px bg-base-content/10" aria-hidden="true" />
          <button
            type="button"
            class="flex size-8 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
            title="About"
            aria-label="About"
            onClick={() => setAboutOpen(true)}
          >
            <TbOutlineInfoCircle size={16} />
          </button>
        </nav>
      </header>
      {/* The traffic providers live above the routes so the charts' rolling
          window keeps filling while the user is on another page (the dock on
          Home picks up a full window on return), and the per-URL summary
          survives navigation the same way. */}
      <main class="flex-1 pt-14">
        <TrafficStatsProvider>
          <UrlStatsProvider>{props.children}</UrlStatsProvider>
        </TrafficStatsProvider>
      </main>
      <AboutModal opened={aboutOpen} setOpened={setAboutOpen} />
      <NotificationsToast />
      <ScrollTopButton />
    </div>
  );
};
