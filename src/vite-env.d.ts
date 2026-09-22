/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** The cargo target platform the Tauri CLI bakes into every build
   *  (darwin / win32 / linux / android). */
  readonly TAURI_ENV_PLATFORM?: string;
  /** Forced UI variant for the build: "desktop" | "mobile". */
  readonly VITE_UI_VARIANT?: string;
}
