import type { RouteRuleType } from "@/types";
import type { RouteAction } from "@/types";

export interface RoutingOption<T extends string> {
  value: T;
  label: string;
}

export const ACTION_ITEMS: Array<RoutingOption<RouteAction>> = [
  { value: "proxy", label: "Proxy" },
  { value: "dpi", label: "DPI" },
  { value: "direct", label: "Direct" },
];

/** What a category rule can be — sing-box domain matchers plus full URLs
 *  (routed by hostname, probed as-is); values are the wire spelling. */
export const RULE_TYPE_ITEMS: Array<RoutingOption<RouteRuleType>> = [
  { value: "domain_suffix", label: "Suffix" },
  { value: "domain", label: "Domain" },
  { value: "domain_keyword", label: "Keyword" },
  { value: "domain_regex", label: "Regex" },
  { value: "url", label: "URL" },
];

const toLabels = <T extends string>(items: Array<RoutingOption<T>>): Record<T, string> =>
  Object.fromEntries(items.map((item) => [item.value, item.label])) as Record<T, string>;

export const ACTION_LABELS = toLabels(ACTION_ITEMS);

export const RULE_TYPE_LABELS = toLabels(RULE_TYPE_ITEMS);
