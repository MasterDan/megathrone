import { createSignal } from "solid-js";

import type { ProxyMode } from "@/types";

import { BUILD_UI_VARIANT } from "@/platform";

const PROFILE_KEY = "megathrone.selectedProfileId";
const MODE_KEY = "megathrone.proxyMode";

const readStoredProfileId = (): number | null => {
  const stored = localStorage.getItem(PROFILE_KEY);
  const parsed = stored === null ? Number.NaN : Number(stored);
  return Number.isFinite(parsed) ? parsed : null;
};

const readStoredMode = (): ProxyMode => {
  const stored = localStorage.getItem(MODE_KEY);
  if (stored !== "system-proxy" && stored !== "tun") return "off";
  // a takeover selection carried over from the desktop variant cannot run
  // on mobile (no VpnService tunnel, no privileged system proxy) — degrade
  // to the local-ports-only mode instead of failing
  return BUILD_UI_VARIANT === "mobile" ? "off" : stored;
};

/** Shared desktop session state: which profile the sidebar has picked and
 *  the proxy mode the next connect uses. The mobile Home page keeps its own
 *  private copy under the same localStorage keys, so the selection survives
 *  switching UI variants. */
const [selectedId, setSelectedId] = createSignal<number | null>(readStoredProfileId());
const [mode, setMode] = createSignal<ProxyMode>(readStoredMode());

export const selectedProfileId = selectedId;
export const proxyMode = mode;

export const selectProfile = (id: number | null) => {
  setSelectedId(id);
  if (id === null) {
    localStorage.removeItem(PROFILE_KEY);
  } else {
    localStorage.setItem(PROFILE_KEY, String(id));
  }
};

export const setProxyMode = (value: ProxyMode) => {
  setMode(value);
  localStorage.setItem(MODE_KEY, value);
};
