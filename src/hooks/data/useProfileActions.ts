import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";

/**
 * Mutations over profiles. `onChanged` fires only after a successful change,
 * so it doubles as "refresh the list" for the caller. The backend also emits
 * `profiles-changed` for every mutation, which covers background updates.
 */
export function useProfileActions(onChanged: () => void) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const run = async (action: () => Promise<unknown>) => {
    setError(null);
    setBusy(true);
    try {
      await action();
      onChanged();
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const importFromUrl = (url: string, name?: string) =>
    run(() =>
      invoke("profile_import_from_url", {
        url,
        name: name?.trim() ? name.trim() : null,
      }),
    );

  const importFromFile = (path: string, name?: string) =>
    run(() =>
      invoke("profile_import_from_file", {
        path,
        name: name?.trim() ? name.trim() : null,
      }),
    );

  const importFromText = (content: string, name: string) =>
    run(() => invoke("profile_import_from_text", { content, name }));

  const update = (profileId: number) => run(() => invoke("profile_update", { profileId }));

  const rename = (profileId: number, newName: string) =>
    run(() => invoke("profile_rename", { profileId, newName }));

  const remove = (profileId: number) => run(() => invoke("profile_delete", { profileId }));

  const setAutoUpdate = (profileId: number, minutes: number | null) =>
    run(() => invoke("profile_set_auto_update", { profileId, minutes }));

  return {
    busy,
    error,
    importFromUrl,
    importFromFile,
    importFromText,
    update,
    rename,
    remove,
    setAutoUpdate,
  };
}
