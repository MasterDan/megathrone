import type { Component } from "solid-js";
import type { JSX } from "solid-js";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import type { ImportFilePick, PlatformApi } from "@megathrone/core/platform";
import { PlatformContext } from "@megathrone/core/platform";

/** The desktop/Android shell: commands ride Tauri's IPC, events its event
 *  bus, the import picker the native file dialog (a path the Rust side can
 *  read itself). */
const api: PlatformApi = {
  kind: "tauri",
  invoke: <T,>(command: string, args?: Record<string, unknown>) => tauriInvoke<T>(command, args),
  listen: <T,>(event: string, handler: (payload: T) => void) =>
    tauriListen<T>(event, (e) => handler(e.payload)),
  pickImportFile: async (): Promise<ImportFilePick | null> => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Subscription", extensions: ["txt", "json", "yaml", "yml", "conf"] }],
    });
    if (typeof selected !== "string") {
      return null;
    }
    const name = selected.split(/[/\\]/).pop() ?? selected;
    return { kind: "path", path: selected, name };
  },
};

export const TauriContext: Component<{ children?: JSX.Element }> = (props) => (
  <PlatformContext.Provider value={api}>{props.children}</PlatformContext.Provider>
);
