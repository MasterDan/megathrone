/** Which UI the build ships. The mobile layout is the default for Android
 *  builds (`tauri android build` sets TAURI_ENV_PLATFORM=android for the
 *  frontend build); everything else is desktop. VITE_UI_VARIANT forces a
 *  variant regardless of the platform — evaluate the other interface from
 *  any build with `VITE_UI_VARIANT=mobile pnpm dev` (or `build`).
 *
 *  Vite statically replaces both `import.meta.env.*` reads with literals,
 *  so this folds into a compile-time constant and the bundler can drop the
 *  dead branch of every `BUILD_UI_VARIANT === …` check after minification.
 *
 *  This decides the strong differences (the router root layout, page
 *  chrome). Small per-component adjustments read the runtime context
 *  instead (contexts/uiVariant.tsx). */
export type UiVariant = "desktop" | "mobile";

const forced = import.meta.env.VITE_UI_VARIANT;

export const BUILD_UI_VARIANT: UiVariant =
  forced === "mobile" || forced === "desktop"
    ? forced
    : import.meta.env.TAURI_ENV_PLATFORM === "android"
      ? "mobile"
      : "desktop";
