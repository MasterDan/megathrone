import type { Component } from "solid-js";
import type { JSX } from "solid-js";

import type { PlatformApi } from "@megathrone/core/platform";
import { PlatformContext } from "@megathrone/core/platform";

/** The browser shell. The UI is served by the megathrone-web console app
 *  and talks to it over plain HTTP:
 *  - `invoke` POSTs JSON args to `/api/<command>` and expects either a JSON
 *    payload (2xx) or `{"error": "..."}` (non-2xx) — the same command names
 *    and camelCase args as the Tauri IPC.
 *  - `listen` subscribes to the server-sent-events stream at `/api/events`
 *    (named events, JSON payloads).
 *  - the import picker is a plain file input: the browser hands over the
 *    content, never a path. */

const ACCEPT = ".txt,.json,.yaml,.yml,.conf";

const api: PlatformApi = {
  kind: "web",

  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const response = await fetch(`/api/${command}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(args ?? {}),
    });
    const payload = await response.json().catch(() => null);
    if (!response.ok) {
      const message =
        payload && typeof payload.error === "string"
          ? payload.error
          : `request failed: ${response.status} ${response.statusText}`;
      throw new Error(message);
    }
    return payload as T;
  },

  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    return new Promise((resolve, reject) => {
      const source = new EventSource("/api/events");
      const forward = (e: MessageEvent) => {
        try {
          handler(JSON.parse(e.data as string) as T);
        } catch {
          // a malformed frame is dropped; the stream stays up
        }
      };
      source.addEventListener(event, forward as EventListener);
      source.onopen = () => resolve(() => source.close());
      source.onerror = () => {
        // EventSource auto-reconnects; only a failed first connect rejects
        if (source.readyState === EventSource.CLOSED) {
          reject(new Error(`event stream closed: ${event}`));
        }
      };
    });
  },

  pickImportFile() {
    return new Promise((resolve) => {
      const input = document.createElement("input");
      input.type = "file";
      input.accept = ACCEPT;
      input.style.display = "none";
      input.addEventListener("change", () => {
        const file = input.files?.[0];
        input.remove();
        if (!file) {
          resolve(null);
          return;
        }
        void file.text().then((text) => {
          resolve({ kind: "text", name: file.name, text });
        });
      });
      // cancelling the dialog never fires "change" in some browsers; clean
      // the hidden node up whenever the tab regains focus with no pick
      window.addEventListener(
        "focus",
        () => setTimeout(() => input.remove(), 500),
        { once: true },
      );
      document.body.append(input);
      input.click();
    });
  },
};

export const WebContext: Component<{ children?: JSX.Element }> = (props) => (
  <PlatformContext.Provider value={api}>{props.children}</PlatformContext.Provider>
);
