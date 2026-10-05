import { createContext, useContext } from "solid-js";

/** A file the user picked for import. The desktop (Tauri) shell yields a
 *  filesystem path the backend can read itself; the browser shell yields
 *  the file's content (the backend never sees a path there). */
export type ImportFilePick =
  | { kind: "path"; path: string; name: string }
  | { kind: "text"; name: string; text: string };

/** Everything platform-specific the UI needs. Implemented per shell:
 *  the Tauri window bridges to the bundled backend over its IPC, the web
 *  build talks to a megathrone-web server over HTTP/SSE. The UI never
 *  imports a platform SDK directly — only this context. */
export interface PlatformApi {
  /** Which shell is running (diagnostics, minor UI tweaks). */
  readonly kind: "tauri" | "web";
  /** Invoke a backend command by name (Tauri command / web API call).
   *  Args use the JS camelCase convention on both platforms. */
  invoke<T = unknown>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** Subscribe to a backend broadcast event; resolves with an unlisten fn. */
  listen<T = unknown>(event: string, handler: (payload: T) => void): Promise<() => void>;
  /** Ask the user to pick a local file for import. Null when cancelled. */
  pickImportFile(): Promise<ImportFilePick | null>;
}

export const PlatformContext = createContext<PlatformApi>();

/** Access the platform bridge. Must be called beneath the shell's provider
 *  (TauriContext / WebContext wrap the whole App). */
export function usePlatform(): PlatformApi {
  const api = useContext(PlatformContext);
  if (!api) {
    throw new Error("usePlatform: no PlatformContext provider above this component");
  }
  return api;
}
