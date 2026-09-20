import type { Component } from "solid-js";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { TbOutlineCheck, TbOutlineCopy, TbOutlineRefresh, TbOutlineX } from "solid-icons/tb";

import { PROFILE_UPDATE_EVENT, RESTART_RESULT_EVENT } from "@/types";
import type { ProfileUpdateNotice, RestartResult } from "@/types";

/** Success toasts fade after this; errors stay until dismissed. */
const SUCCESS_TIMEOUT_MS = 2500;
/** The transient "updating…" notice fades a bit later on its own — the
 *  outcome toast replaces it whenever the update finishes first. */
const INFO_TIMEOUT_MS = 4000;
/** Cap of simultaneously visible entries (the oldest fall off). */
const STACK_LIMIT = 4;

type Severity = "info" | "success" | "error";

interface ToastEntry {
  id: number;
  severity: Severity;
  message: string;
  /** set for profile-update entries: the outcome toast replaces this
   *  profile's lingering "started" notice instead of stacking onto it */
  profileId?: number;
}

let nextId = 0;

const copyText = async (text: string) => {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // clipboard may be unavailable (no focus / permission) — nothing to do
  }
};

/**
 * The global notification toast — one stack for every global event source:
 * restart results (user-initiated reconfigurations) and profile-update
 * notices (started / success / error, background refreshes included).
 * Info and success entries hide themselves; an error stays until dismissed
 * so its text can be read and copied. Mounted once in the layout — every
 * page sees the same toasts.
 */
export const NotificationsToast: Component = () => {
  const [toasts, setToasts] = createSignal<ToastEntry[]>([]);

  const dismiss = (id: number) => {
    setToasts((list) => list.filter((toast) => toast.id !== id));
  };

  const push = (severity: Severity, message: string, profileId?: number) => {
    const entry: ToastEntry = { id: nextId++, severity, message, profileId };
    setToasts((list) => {
      // the outcome replaces the same profile's "started" notice
      const withoutStarted = profileId == null || severity === "info"
        ? list
        : list.filter((toast) => toast.profileId !== profileId || toast.severity !== "info");
      return [...withoutStarted.slice(-(STACK_LIMIT - 1)), entry];
    });
    if (severity !== "error") {
      window.setTimeout(
        () => dismiss(entry.id),
        severity === "info" ? INFO_TIMEOUT_MS : SUCCESS_TIMEOUT_MS,
      );
    }
  };

  onMount(() => {
    let disposed = false;
    const keep = (unlisten: () => void) => {
      if (disposed) {
        unlisten();
      } else {
        onCleanup(() => unlisten());
      }
    };

    void listen<RestartResult>(RESTART_RESULT_EVENT, (event) => {
      push(event.payload.ok ? "success" : "error", event.payload.message);
    }).then((unlisten) => keep(unlisten));

    void listen<ProfileUpdateNotice>(PROFILE_UPDATE_EVENT, (event) => {
      const notice = event.payload;
      switch (notice.kind) {
        case "started":
          push("info", `Updating ${notice.profileName}…`, notice.profileId);
          break;
        case "success":
          push(
            "success",
            `${notice.profileName} — ${notice.message ?? "updated"}`,
            notice.profileId,
          );
          break;
        case "error":
          push(
            "error",
            `${notice.profileName}: ${notice.message ?? "update failed"}`,
            notice.profileId,
          );
          break;
      }
    }).then((unlisten) => keep(unlisten));

    onCleanup(() => {
      disposed = true;
    });
  });

  return (
    <div class="toast toast-bottom toast-center z-[70] gap-2 pb-24">
      <For each={toasts()}>
        {(toast) => (
          <div
            class="alert max-w-md items-start py-2.5 text-sm shadow-lg"
            classList={{
              "alert-info": toast.severity === "info",
              "alert-success": toast.severity === "success",
              "alert-error": toast.severity === "error",
            }}
            role={toast.severity === "error" ? "alert" : "status"}
          >
            <Show
              when={toast.severity === "success"}
              fallback={
                <Show
                  when={toast.severity === "info"}
                  fallback={<TbOutlineX size={18} class="shrink-0" />}
                >
                  <TbOutlineRefresh size={18} class="shrink-0 animate-spin" />
                </Show>
              }
            >
              <TbOutlineCheck size={18} class="shrink-0" />
            </Show>
            <span class="max-w-sm whitespace-pre-wrap break-words">{toast.message}</span>
            <Show when={toast.severity === "error"}>
              <button
                type="button"
                class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 transition-colors hover:bg-base-content/20"
                title="Copy the error"
                aria-label="Copy the error"
                onClick={() => void copyText(toast.message)}
              >
                <TbOutlineCopy size={14} />
              </button>
            </Show>
            <button
              type="button"
              class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 transition-colors hover:bg-base-content/20"
              title="Dismiss"
              aria-label="Dismiss"
              onClick={() => dismiss(toast.id)}
            >
              <TbOutlineX size={14} />
            </button>
          </div>
        )}
      </For>
    </div>
  );
};
