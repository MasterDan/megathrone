import type { Component } from "solid-js";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { A, useNavigate, useParams } from "@solidjs/router";
import {
  TbOutlineAlertTriangle,
  TbOutlineArrowLeft,
  TbOutlineCircleCheck,
  TbOutlineFoldersOff,
  TbOutlineListTree,
  TbOutlinePencil,
  TbOutlinePlus,
  TbOutlineTrash,
  TbOutlineX,
} from "solid-icons/tb";

import { Empty } from "@/components/common/Empty";
import { OptionTabs } from "@/components/common/daisy-ui/OptionTabs";
import { useIsMobileUi } from "@/contexts/uiVariant";
import { CategoryModal } from "@/components/settings/CategoryModal";
import { ACTION_LABELS, RULE_TYPE_ITEMS } from "@/components/settings/routingOptions";
import { useTestSites } from "@/hooks/data/useTestSites";
import type { CategoryRule, RouteRuleType, TestSiteCategory } from "@/types";

/** Types that cannot produce a probe URL (they join routing only). */
const notProbeable = (type: RouteRuleType) =>
  type === "domain_keyword" || type === "domain_regex";

/** One rule card: the value, what it is (the sing-box matcher kind) and
 *  whether it joins the tests — all editable in place. */
const RuleCard: Component<{
  rule: CategoryRule;
  busy: boolean;
  onUpdate: (ruleId: number, ruleType: RouteRuleType, value: string) => Promise<boolean>;
  onToggleTest: (ruleId: number, testEnabled: boolean) => void;
  onDelete: (ruleId: number) => void;
}> = (props) => {
  const [draft, setDraft] = createSignal(props.rule.value);

  // drafts follow the stored value unless the user is mid-edit
  createEffect(() => {
    setDraft((current) => (current === props.rule.value ? current : props.rule.value));
  });

  const dirty = () => draft().trim() !== props.rule.value;

  const save = () => {
    const value = draft().trim();
    if (!value || props.busy || !dirty()) {
      return;
    }
    void props.onUpdate(props.rule.id, props.rule.ruleType, value);
  };

  const switchType = (type: RouteRuleType) => {
    if (props.busy || type === props.rule.ruleType) {
      return;
    }
    // re-validates the current value for the new type — a full URL cannot
    // become a bare-domain rule (and vice versa) without editing the value
    void props.onUpdate(props.rule.id, type, draft().trim());
  };

  return (
    <li class="space-y-2 rounded-xl bg-base-200/60 p-3">
      <div class="flex items-center gap-1.5">
        <input
          type="text"
          class="input input-bordered input-sm min-w-0 flex-1 font-mono text-xs"
          classList={{ "input-error": dirty() && !draft().trim() }}
          value={draft()}
          disabled={props.busy}
          onInput={(event) => setDraft(event.currentTarget.value)}
          onBlur={save}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              save();
            }
            if (event.key === "Escape") {
              setDraft(props.rule.value);
            }
          }}
        />
        <Show
          when={dirty()}
          fallback={
            <button
              type="button"
              class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-error transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
              title="Remove rule"
              aria-label={`Remove ${props.rule.value}`}
              disabled={props.busy}
              onClick={() => props.onDelete(props.rule.id)}
            >
              <TbOutlineTrash size={14} />
            </button>
          }
        >
          <button
            type="button"
            class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-success transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
            title="Save rule"
            aria-label="Save rule"
            disabled={props.busy || !draft().trim()}
            onClick={save}
          >
            <TbOutlineCircleCheck size={15} />
          </button>
          <button
            type="button"
            class="flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
            title="Discard changes"
            aria-label="Discard changes"
            disabled={props.busy}
            onClick={() => setDraft(props.rule.value)}
          >
            <TbOutlineX size={15} />
          </button>
        </Show>
      </div>
      <div class="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        <OptionTabs
          items={RULE_TYPE_ITEMS}
          value={props.rule.ruleType}
          disabled={props.busy}
          onChange={(value) => switchType(value)}
        />
        <label
          class="flex cursor-pointer items-center gap-2 text-xs text-base-content/60"
          classList={{ "cursor-not-allowed opacity-50": notProbeable(props.rule.ruleType) }}
          title={
            notProbeable(props.rule.ruleType)
              ? "Keyword and regex rules join routing only — they cannot be probed"
              : "Join the DPI strategy test and the endpoint deep probe"
          }
        >
          Testing
          <input
            type="checkbox"
            class="toggle toggle-sm toggle-primary"
            checked={props.rule.testEnabled}
            disabled={props.busy || notProbeable(props.rule.ruleType)}
            onChange={(event) =>
              props.onToggleTest(props.rule.id, event.currentTarget.checked)
            }
          />
        </label>
      </div>
    </li>
  );
};

/** The rule editor of one category (Settings → Routing → category). Every
 *  rule is a card: the value, the matcher kind and a Testing toggle —
 *  only toggled-on rules join the DPI strategy test and the endpoint deep
 *  probe. */
export const CategoryPage: Component = () => {
  const params = useParams();
  const navigate = useNavigate();
  const sites = useTestSites();
  const isMobile = useIsMobileUi();

  const [renameOpened, setRenameOpened] = createSignal(false);
  const [confirmDelete, setConfirmDelete] = createSignal(false);
  const [newValue, setNewValue] = createSignal("");
  const [newType, setNewType] = createSignal<RouteRuleType>("domain_suffix");

  const category = createMemo<TestSiteCategory | null>(() => {
    const id = Number(params.id);
    return sites.categories().find((entry) => entry.id === id) ?? null;
  });

  const submitRename = (name: string) => {
    const current = category();
    if (!current) {
      return Promise.resolve(false);
    }
    return sites.renameCategory(current.id, name);
  };

  const remove = () => {
    const current = category();
    if (!current || sites.busy()) {
      return;
    }
    if (!confirmDelete()) {
      setConfirmDelete(true);
      return;
    }
    void sites.deleteCategory(current.id).then((ok) => {
      if (ok) {
        navigate("/settings?tab=routing");
      }
    });
  };

  const submitRule = () => {
    const current = category();
    const value = newValue().trim();
    if (!current || !value || sites.busy()) {
      return;
    }
    void sites.addRule(current.id, newType(), value).then((ok) => {
      if (ok) {
        setNewValue("");
      }
    });
  };

  const BackAction: Component = () => (
    <A
      href="/settings?tab=routing"
      class="inline-flex h-8 cursor-pointer items-center justify-center gap-1 rounded-full bg-base-content/10 px-3 text-sm font-medium whitespace-nowrap text-base-content transition-colors hover:bg-base-content/20 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary"
    >
      <TbOutlineArrowLeft size={16} />
      Back to routing
    </A>
  );

  return (
    <div class="container mx-auto max-w-2xl space-y-4 p-4">
      <Show when={sites.loading()}>
        <div class="flex justify-center py-16">
          <span class="loading loading-spinner text-primary" />
        </div>
      </Show>

      <Show when={!sites.loading() && !category()}>
        <Empty
          icon={TbOutlineFoldersOff}
          title="Category not found"
          description="It may have been deleted."
          action={BackAction}
        />
      </Show>

      <Show when={category()}>
        {(current) => (
          <>
            <div
              classList={{
                "sticky z-40 -mx-4 rounded-2xl border border-base-content/10 bg-base-100/60 px-4 py-2.5 shadow-sm backdrop-blur-md": true,
                "top-14": isMobile(),
                "top-0": !isMobile(),
              }}
            >
              <div class="grid grid-cols-[1fr_auto_1fr] items-center gap-x-2 gap-y-1">
                <div class="flex min-w-0 items-center gap-2">
                  <A
                    href="/settings?tab=routing"
                    class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content"
                    title="Back to routing"
                    aria-label="Back to routing"
                  >
                    <TbOutlineArrowLeft size={16} />
                  </A>
                  <div class="min-w-0">
                    <h1 class="truncate text-lg font-bold" title={current().name}>
                      {current().name}
                    </h1>
                    <p class="truncate text-xs text-base-content/50">
                      {current().rules.length} rules · routed as{" "}
                      {ACTION_LABELS[current().action]}
                    </p>
                  </div>
                </div>
                <span class="hidden justify-self-center text-xs text-base-content/40 sm:block">
                  switch the outbound on the Routing list
                </span>
                <div class="flex items-center justify-self-end gap-1">
                  <button
                    type="button"
                    class="flex size-8 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-base-content/70 transition-colors hover:bg-base-content/20 hover:text-base-content disabled:pointer-events-none disabled:opacity-40"
                    title="Rename category"
                    aria-label="Rename category"
                    disabled={sites.busy()}
                    onClick={() => setRenameOpened(true)}
                  >
                    <TbOutlinePencil size={16} />
                  </button>
                  <Show
                    when={!confirmDelete()}
                    fallback={
                      <button
                        type="button"
                        class="inline-flex h-8 cursor-pointer select-none items-center justify-center gap-1 rounded-full bg-error/20 px-3 text-sm font-medium whitespace-nowrap text-error transition-colors hover:bg-error/30 disabled:pointer-events-none disabled:opacity-40"
                        disabled={sites.busy()}
                        onClick={remove}
                      >
                        Delete?
                      </button>
                    }
                  >
                    <button
                      type="button"
                      class="flex size-8 cursor-pointer items-center justify-center rounded-full bg-base-content/10 text-error transition-colors hover:bg-base-content/20 disabled:pointer-events-none disabled:opacity-40"
                      title="Delete category"
                      aria-label="Delete category"
                      disabled={sites.busy()}
                      onClick={() => setConfirmDelete(true)}
                    >
                      <TbOutlineTrash size={16} />
                    </button>
                  </Show>
                </div>
              </div>
            </div>

            <Show when={sites.error() && !renameOpened()}>
              <div class="alert alert-error py-2 text-sm">
                <span class="break-all">{sites.error()}</span>
                <TbOutlineX size={16} />
              </div>
            </Show>

            <Show
              when={current().rules.length > 0}
              fallback={
                <div class="flex items-center gap-2 rounded-xl bg-base-200/60 px-3 py-4 text-sm text-base-content/50">
                  <TbOutlineAlertTriangle size={16} class="shrink-0" />
                  No rules yet — add the first one below.
                </div>
              }
            >
              <ul class="space-y-2">
                <For each={current().rules}>
                  {(rule) => (
                    <RuleCard
                      rule={rule}
                      busy={sites.busy()}
                      onUpdate={(ruleId, ruleType, value) =>
                        sites.updateRule(ruleId, ruleType, value)
                      }
                      onToggleTest={(ruleId, testEnabled) =>
                        void sites.setRuleTest(ruleId, testEnabled)
                      }
                      onDelete={(ruleId) => void sites.deleteRule(ruleId)}
                    />
                  )}
                </For>
              </ul>
            </Show>

            <div class="card bg-base-content/5 shadow-sm">
              <div class="card-body gap-2 p-4">
                <h2 class="flex items-center gap-2 text-sm font-semibold">
                  <TbOutlineListTree size={16} class="text-base-content/50" />
                  Add rule
                </h2>
                <div class="flex items-center gap-1.5">
                  <input
                    type="text"
                    class="input input-bordered input-sm min-w-0 flex-1 font-mono text-xs"
                    placeholder="example.com or https://example.com/check"
                    value={newValue()}
                    disabled={sites.busy()}
                    onInput={(event) => setNewValue(event.currentTarget.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        submitRule();
                      }
                    }}
                  />
                  <button
                    type="button"
                    class="flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full bg-primary/20 text-primary transition-colors hover:bg-primary/30 disabled:pointer-events-none disabled:opacity-40"
                    title="Add rule"
                    aria-label="Add rule"
                    disabled={sites.busy() || !newValue().trim()}
                    onClick={submitRule}
                  >
                    <TbOutlinePlus size={16} />
                  </button>
                </div>
                <OptionTabs
                  items={RULE_TYPE_ITEMS}
                  value={newType()}
                  disabled={sites.busy()}
                  onChange={setNewType}
                />
                <p class="text-xs text-base-content/50">
                  Bare-domain types route the host and every subdomain and are probed as{" "}
                  <span class="font-mono">https://&lt;value&gt;/</span>; URL rules are probed
                  as-is and route by their hostname. Keyword and regex rules join routing
                  only.
                </p>
              </div>
            </div>

            <CategoryModal
              opened={renameOpened}
              setOpened={setRenameOpened}
              busy={sites.busy}
              error={sites.error}
              category={() => current()}
              onSubmit={submitRename}
            />
          </>
        )}
      </Show>
    </div>
  );
};
