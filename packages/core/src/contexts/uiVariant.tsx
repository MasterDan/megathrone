import type { Accessor, ParentComponent } from "solid-js";
import { createContext, createSignal, useContext } from "solid-js";

import { BUILD_UI_VARIANT, type UiVariant } from "@/platform";

const STORAGE_KEY = "megathrone.uiVariant";

const readQueryParam = (): UiVariant | null => {
  const value = new URLSearchParams(window.location.search).get("ui");
  return value === "mobile" || value === "desktop" ? value : null;
};

const readStored = (): UiVariant | null => {
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored === "mobile" || stored === "desktop" ? stored : null;
};

const [variant] = createSignal<UiVariant>(
  readQueryParam() ?? readStored() ?? BUILD_UI_VARIANT,
);

const UiVariantContext = createContext<Accessor<UiVariant>>(variant);

/**
 * The UI variant this session renders as. The build decides the layout
 * (platform.ts), but a desktop build can still present (and evaluate) the
 * mobile interface: `?ui=mobile` wins for the session, a stored override
 * (`megathrone.uiVariant`) survives reloads, and the build variant is the
 * fallback. Components with strongly different layouts branch on the build
 * constant instead — this context is for small, runtime-visible adjustments
 * (a card rendering a bit differently, a sticky offset).
 */
export const UiVariantProvider: ParentComponent = (props) => (
  <UiVariantContext.Provider value={variant}>{props.children}</UiVariantContext.Provider>
);

export const useUiVariant = (): Accessor<UiVariant> => useContext(UiVariantContext);

export const useIsMobileUi = (): Accessor<boolean> => {
  const variant = useUiVariant();
  return () => variant() === "mobile";
};
