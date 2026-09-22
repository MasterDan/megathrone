import type { Component } from "solid-js";
import { createSignal } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { TbOutlineShieldLock } from "solid-icons/tb";

import { DesktopPage } from "@/components/layout/desktop/DesktopPage";
import { DpiSection } from "@/components/settings/DpiSection";

/**
 * DPI strategies as a first-class desktop page: the master toggle and port
 * above the strategy list (on mobile these are Settings tabs). A
 * ?strategy=<id> deep link (from a status link) scrolls that strategy into
 * view once, then cleans the URL.
 */
export const DesktopDpi: Component = () => {
  const location = useLocation();
  const navigate = useNavigate();
  const requested = Number(new URLSearchParams(location.search).get("strategy"));
  const [revealStrategy, setRevealStrategy] = createSignal<number | null>(
    Number.isInteger(requested) && requested > 0 ? requested : null,
  );

  return (
    <DesktopPage
      title={
        <h1 class="flex items-center gap-2 text-lg font-bold">
          <TbOutlineShieldLock size={20} class="text-base-content/60" />
          DPI Strategies
        </h1>
      }
    >
      <div class="mx-auto max-w-2xl">
        <DpiSection
          revealStrategyId={revealStrategy()}
          onRevealDone={() => {
            setRevealStrategy(null);
            navigate("/dpi", { replace: true });
          }}
        />
      </div>
    </DesktopPage>
  );
};
