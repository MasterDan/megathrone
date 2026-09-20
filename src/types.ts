export interface ProfileSummary {
  id: number;
  name: string;
  sourceUrl: string | null;
  sourcePath: string | null;
  autoUpdateMinutes: number | null;
  lastFetchedAt: string | null;
  itemCount: number;
  skippedCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface EndpointItem {
  id: number;
  tag: string;
  protocol: string;
  server: string | null;
  serverPort: number | null;
  available: boolean | null;
  latencyMs: number | null;
  upBytes: number;
  downBytes: number;
  speedBps: number | null;
  /** deep-probed proxy-category URLs reachable through this endpoint /
   *  probed total (0/0 — not deep-probed in the last scan) */
  urlOk: number;
  urlTotal: number;
}

/** A byedpi (ciadpi) strategy: a raw argument line ("-s2 -d2"). */
export interface DpiStrategy {
  id: number;
  name: string;
  args: string;
  isActive: boolean;
  /** dpi-category URLs reachable through this strategy / configured total
   *  (`tested === 0` — never tested) */
  urlOk: number;
  urlTotal: number;
  tested: number;
}

/** DPI tunnel settings (Settings → DPI). */
export interface DpiSettings {
  port: number;
  /** master toggle: off — sessions are proxy-only, the byedpi tunnel never starts */
  enabled: boolean;
}

/** Where matched traffic goes while connected. */
export type RouteAction = "proxy" | "dpi" | "direct";

/** What a category rule is: a sing-box domain matcher (wire/DB spelling),
 *  or a full URL (routed by its hostname, probed as-is). Keyword/regex
 *  rules join routing only — they cannot be probed. */
export type RouteRuleType =
  | "domain_suffix"
  | "domain"
  | "domain_keyword"
  | "domain_regex"
  | "url";

/** One routing rule inside a category (Settings → Routing). */
export interface CategoryRule {
  id: number;
  ruleType: RouteRuleType;
  value: string;
  /** joins the DPI strategy test and the endpoint deep probe */
  testEnabled: boolean;
}

/** A category as routing sees it: its rules follow `action`. */
export interface RouteCategory {
  id: number;
  name: string;
  action: RouteAction;
  rules: Array<{ ruleType: RouteRuleType; value: string }>;
}

/** Routed categories plus the fallback action for everything unmatched. */
export interface RoutingConfig {
  categories: RouteCategory[];
  fallback: RouteAction;
}

/** General settings (Settings → General): the automatic-strategy
 *  supervisor cadences, the close-to-tray behavior and the raw local
 *  proxy port. */
export interface GeneralSettings {
  /** Minutes between round robin rotations to the next reachable endpoint. */
  rotationMinutes: number;
  /** Minutes between background re-checks of the scan-reachable endpoints. */
  recheckMinutes: number;
  /** Whether closing the window hides it to the tray instead of quitting. */
  closeToTray: boolean;
  /** Deep-probe pass rate (percent) an endpoint must clear to stay
   *  selectable by the automatic strategies; 0 disables the floor. */
  minAvailabilityPercent: number;
  /** Whether the raw local proxy port is opened alongside a session. */
  rawProxyEnabled: boolean;
  /** The configured port of the raw local proxy (the actual port of the
   *  running session is in `ConnectionSnapshot.rawProxyPort`). */
  rawProxyPort: number;
}

/** Present in a connection snapshot while the DPI tunnel runs. */
export interface DpiSnapshot {
  strategy: string;
  port: number;
}

/** One deep-probed category URL's latest outcome for an endpoint. */
export interface EndpointUrlStatus {
  urlId: number;
  category: string;
  url: string;
  available: boolean | null;
  latencyMs: number | null;
}

export interface ItemDetail {
  raw: string;
  outboundJson: string;
}

export const PROFILES_CHANGED_EVENT = "profiles-changed";

export type ProfilesChangedKind =
  | "content"
  | "meta"
  | "latency"
  | "selection"
  | "updating"
  | "update-done";

export interface ProfilesChangedPayload {
  profileId: number | null;
  kind: ProfilesChangedKind;
}

export const LATENCY_PROGRESS_EVENT = "latency-progress";

/** How the profile's live endpoint is chosen (the strategy tabs on the
 *  endpoints page). Wire spelling matches the Rust constants. */
export type SelectionMode = "round_robin" | "fastest" | "most_available" | "manual";

export const SELECTION_MODE_LABELS: Record<SelectionMode, string> = {
  round_robin: "Round Robin",
  fastest: "Fastest",
  most_available: "Most Available",
  manual: "Endpoint",
};

export const CONNECTION_CHANGED_EVENT = "connection-changed";

/** Emitted after a user-initiated live reconfiguration (endpoint pick,
 *  strategy switch, DPI change, profile switch): successes hide fast in
 *  the toast, errors stay until dismissed. */
export const RESTART_RESULT_EVENT = "restart-result";

export interface RestartResult {
  ok: boolean;
  message: string;
}

/** Emitted around every profile update (the manual button and the
 *  background scheduler alike): started → success/error, so the global
 *  toast can narrate background refreshes. Errors stay until dismissed. */
export const PROFILE_UPDATE_EVENT = "profile-update";

export type ProfileUpdateKind = "started" | "success" | "error";

export interface ProfileUpdateNotice {
  profileId: number;
  profileName: string;
  kind: ProfileUpdateKind;
  message: string | null;
}

/** One routing rule inside a category (Settings → Routing). */
export interface CategoryRule {
  id: number;
  ruleType: RouteRuleType;
  value: string;
  /** joins the DPI strategy test and the endpoint deep probe */
  testEnabled: boolean;
}

/** A URL category. Its `action` (Settings → Routing) decides where its
 *  rules go while connected and which test probes them. */
export interface TestSiteCategory {
  id: number;
  name: string;
  action: RouteAction;
  rules: CategoryRule[];
}

export const TEST_SITES_CHANGED_EVENT = "test-sites-changed";

/** One strategy's outcome of a running/finished DPI strategy test. */
export interface DpiTestResult {
  strategyId: number;
  urlOk: number;
  urlTotal: number;
  /** set when the strategy never got a tunnel up (nothing was stored) */
  error: string | null;
}

/** Emitted per tested strategy while a DPI strategy test runs. */
export interface DpiTestProgress {
  done: number;
  total: number;
  results: DpiTestResult[];
}

/** Snapshot of a running strategy test; lets a re-mounted page adopt it. */
export interface DpiTestStatus {
  done: number;
  total: number;
}

/** One test site's latest outcome for a strategy; `ok` is null when the
 *  pair has not been probed yet. */
export interface DpiSiteStatus {
  categoryId: number;
  category: string;
  urlId: number;
  url: string;
  ok: boolean | null;
}

export const DPI_TEST_PROGRESS_EVENT = "dpi-test-progress";

export const DPI_TEST_FINISHED_EVENT = "dpi-test-finished";

/** How traffic is routed through the running proxy. */
export type ProxyMode = "off" | "system-proxy" | "tun";

/** State of the long-lived sing-box proxy; `lastError` carries the reason of
 *  an unexpected exit or a failed connect attempt. `dpi` is set while the
 *  byedpi tunnel runs alongside sing-box. */
export interface ConnectionSnapshot {
  connected: boolean;
  profileId: number | null;
  profileName: string | null;
  endpointTag: string | null;
  mode: ProxyMode | null;
  mixedPort: number | null;
  /** The raw local proxy port the session bakes (`null` — off, or a
   *  DPI-only session with no endpoint); point apps that support a proxy
   *  at `127.0.0.1:<port>` to send everything through the selected
   *  endpoint. */
  rawProxyPort: number | null;
  dpi: DpiSnapshot | null;
  lastError: string | null;
}

/** One endpoint's availability probe result (batched in `LatencyProgress`). */
export interface EndpointLatency {
  id: number;
  available: boolean;
  latencyMs: number | null;
  urlOk: number;
  urlTotal: number;
}

/** Emitted once per probe batch, not per endpoint, to keep event traffic low. */
export interface LatencyProgress {
  profileId: number;
  done: number;
  total: number;
  results: EndpointLatency[];
}

/** Final result of a whole-profile availability scan. */
export interface LatencySummary {
  total: number;
  reachable: number;
}

/** Snapshot of a running scan; lets a re-mounted page adopt it. */
export interface LatencyStatus {
  done: number;
  total: number;
}

/** Cumulative per-outbound byte counters of the running session, emitted
 *  ~every second while connected (up = client → internet). */
export interface TrafficStats {
  proxyUp: number;
  proxyDown: number;
  dpiUp: number;
  dpiDown: number;
  directUp: number;
  directDown: number;
}

export const TRAFFIC_STATS_EVENT = "traffic-stats";

/** One row of the per-URL session summary: the host that was reached, the
 *  tunnel it rode and its request counts — `requests` counts every
 *  connection seen (open ones included), `ok`/`failed` split the closed
 *  ones by whether a response ever came back. */
export interface UrlStatEntry {
  host: string;
  tunnel: "proxy" | "dpi" | "direct";
  requests: number;
  ok: number;
  failed: number;
}
