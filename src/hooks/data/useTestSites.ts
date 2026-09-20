import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { TEST_SITES_CHANGED_EVENT } from "@/types";
import type { CategoryRule, RouteAction, RouteRuleType, TestSiteCategory } from "@/types";

/** Equal enough to keep the previous object (and its DOM) alive through a
 *  refresh — fresh objects would re-create every row in `For` and read as
 *  jitter. */
const sameRule = (a: CategoryRule, b: CategoryRule): boolean =>
  a.id === b.id &&
  a.ruleType === b.ruleType &&
  a.value === b.value &&
  a.testEnabled === b.testEnabled;

const sameCategory = (a: TestSiteCategory, b: TestSiteCategory): boolean =>
  a.id === b.id &&
  a.name === b.name &&
  a.action === b.action &&
  a.rules.length === b.rules.length &&
  a.rules.every((rule, index) => sameRule(rule, b.rules[index]));

/**
 * URL categories and their routing rules (Settings → Routing). Mutations
 * return true on success; the backend also emits `test-sites-changed` so
 * every mounted consumer (including this hook) refreshes.
 */
export function useTestSites() {
  const [categories, setCategories] = createSignal<TestSiteCategory[]>([]);
  const [loading, setLoading] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const refresh = async () => {
    try {
      const fresh = await invoke<TestSiteCategory[]>("sites_list");
      setError(null);
      setCategories((previous) =>
        fresh.map((category) => {
          const kept = previous.find((candidate) => candidate.id === category.id);
          return kept && sameCategory(kept, category) ? kept : category;
        }),
      );
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  };

  const run = async (action: () => Promise<unknown>) => {
    setError(null);
    setBusy(true);
    try {
      await action();
      await refresh();
      return true;
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const addCategory = (name: string) => run(() => invoke("sites_add_category", { name }));

  const renameCategory = (categoryId: number, name: string) =>
    run(() => invoke("sites_rename_category", { categoryId, name }));

  const deleteCategory = (categoryId: number) =>
    run(() => invoke("sites_delete_category", { categoryId }));

  /** Optimistic, without the global busy flag: flipping a switch must not
   *  disable every control and re-render the list around it (that was the
   *  UI jitter). The backend's `test-sites-changed` event brings the
   *  authoritative list right after; on failure the flip is reverted. */
  const setAction = async (categoryId: number, action: RouteAction) => {
    setError(null);
    const previous = categories().find((category) => category.id === categoryId)?.action;
    const flip = (to: RouteAction) =>
      setCategories((current) =>
        current.map((category) =>
          category.id === categoryId ? { ...category, action: to } : category,
        ),
      );
    flip(action);
    try {
      await invoke("sites_set_action", { categoryId, action });
      return true;
    } catch (cause) {
      if (previous) {
        flip(previous);
      }
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    }
  };

  const addRule = (categoryId: number, ruleType: RouteRuleType, value: string) =>
    run(() => invoke("sites_add_rule", { categoryId, ruleType, value }));

  const updateRule = (ruleId: number, ruleType: RouteRuleType, value: string) =>
    run(() => invoke("sites_update_rule", { ruleId, ruleType, value }));

  /** Same optimistic no-busy treatment as action flips. */
  const setRuleTest = async (ruleId: number, testEnabled: boolean) => {
    setError(null);
    const flip = () =>
      setCategories((current) =>
        current.map((category) => ({
          ...category,
          rules: category.rules.map((rule) =>
            rule.id === ruleId ? { ...rule, testEnabled } : rule,
          ),
        })),
      );
    flip();
    try {
      await invoke("sites_set_rule_test", { ruleId, testEnabled });
      return true;
    } catch (cause) {
      flip();
      setError(cause instanceof Error ? cause.message : String(cause));
      return false;
    }
  };

  const deleteRule = (ruleId: number) => run(() => invoke("sites_delete_rule", { ruleId }));

  onMount(() => {
    void refresh();
    let disposed = false;
    void listen(TEST_SITES_CHANGED_EVENT, () => void refresh()).then((unlisten) => {
      if (disposed) {
        unlisten();
      } else {
        onCleanup(() => unlisten());
      }
    });
    onCleanup(() => {
      disposed = true;
    });
  });

  return {
    categories,
    loading,
    busy,
    error,
    addCategory,
    renameCategory,
    deleteCategory,
    setAction,
    addRule,
    updateRule,
    setRuleTest,
    deleteRule,
    refresh,
  };
}
