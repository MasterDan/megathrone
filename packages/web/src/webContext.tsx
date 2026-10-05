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
 *    (named events, JSON payloads). Every subscription shares ONE
 *    EventSource: browsers cap parallel connections per origin, so a
 *    stream per listener would exhaust the pool. The browser reconnects a
 *    dropped stream on its own; a hard close (server gone) retries on a
 *    timer, and the last subscription out closes the stream.
 *  - the import picker is a plain file input: the browser hands over the
 *    content, never a path. */

const ACCEPT = ".txt,.json,.yaml,.yml,.conf";
const RECONNECT_DELAY_MS = 3000;

type Handler = (payload: unknown) => void;

const listeners = new Map<string, Set<Handler>>();
let source: EventSource | null = null;
let reconnectTimer: number | null = null;

function dispatchFrame(frame: MessageEvent) {
  let payload: unknown;
  try {
    payload = JSON.parse(frame.data as string);
  } catch {
    return; // a malformed frame is dropped; the stream stays up
  }
  for (const handler of listeners.get(frame.type) ?? []) {
    handler(payload);
  }
}

function connect() {
  if (source || reconnectTimer !== null) {
    return;
  }
  const stream = new EventSource("/api/events");
  source = stream;
  for (const event of listeners.keys()) {
    stream.addEventListener(event, dispatchFrame as EventListener);
  }
  stream.onerror = () => {
    // transient failures reconnect on their own (named listeners survive
    // a reconnect); only a hard close needs our own retry
    if (stream.readyState === EventSource.CLOSED) {
      source = null;
      reconnectTimer = window.setTimeout(connect, RECONNECT_DELAY_MS);
    }
  };
}

function disconnectIfIdle() {
  if (listeners.size === 0) {
    if (reconnectTimer !== null) {
      window.clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    source?.close();
    source = null;
  }
}

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
    let handlers = listeners.get(event);
    if (!handlers) {
      handlers = new Set();
      listeners.set(event, handlers);
      source?.addEventListener(event, dispatchFrame as EventListener);
    }
    handlers.add(handler as Handler);
    connect();
    return Promise.resolve(() => {
      const current = listeners.get(event);
      if (!current) {
        return;
      }
      current.delete(handler as Handler);
      if (current.size === 0) {
        listeners.delete(event);
        source?.removeEventListener(event, dispatchFrame as EventListener);
      }
      disconnectIfIdle();
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
