export interface StrategyInfo {
  name: string;
  file_name: string;
  active: boolean;
  args_preview: string;
}

export interface GeneratedInfo {
  name: string;
  parent: string;
  score: number | null;
  active: boolean;
  args_preview: string;
}

export interface AppStatus {
  app_version: string;
  strategies: StrategyInfo[];
  generated: GeneratedInfo[];
  current_strategy: string;
  running: boolean;
  installed_version: string | null;
  runtime_valid: boolean;
}

export interface ConfigView {
  autostart: boolean;
  check_updates: boolean;
  check_app_updates: boolean;
  flashscore_bypass: boolean;
  game_filter: string;
  runtime_version: string;
  runtime_path: string;
  theme: string;
  bench_budget: number;
  bench_auto_apply: boolean;
  tgws: TgwsConfigView;
  xray: XrayConfigView;
}

export interface TgwsConfigView {
  autostart: boolean;
  host: string;
  port: number;
  secret: string;
  installed_version: string;
}

export type TgwsState = "stopped" | "starting" | "running" | "stopping" | "error" | "not_installed";

export interface TgwsStatus {
  state: TgwsState;
  pid: number | null;
  last_error: string | null;
  installed_version: string;
  binary_present: boolean;
  port_conflict: boolean;
  client_connections_total: number;
}

export interface TgwsDiagnosis {
  binary: boolean;
  supervisor_running: boolean;
  process_running: boolean;
  port_open: boolean;
  port_conflict: boolean;
  clients: number;
  log_errors: number;
  last_error_line: string | null;
  last_error: string | null;
  hint: string;
  kind: "ok" | "binary" | "port" | "stopped" | "not_connected" | "blocked" | "unknown";
}

export type XrayState = "stopped" | "starting" | "running" | "stopping" | "error" | "not_installed";

export interface XrayStatus {
  state: XrayState;
  pid: number | null;
  last_error: string | null;
  installed_version: string;
  binary_present: boolean;
  socks_port: number;
  http_port: number;
  selected: number;
  profile_name: string;
  client_connections: number;
  system_proxy: boolean;
  fetched_at: number;
  subscription_upload: number;
  subscription_download: number;
  subscription_total: number;
  subscription_expire: number;
  subscription_page: string;
  has_previous: boolean;
}

export interface XrayProfile {
  index: number;
  name: string;
  host: string;
  port: number;
  scheme: string;
  ping_ms: number | null;
}

/** A host the page referenced, with how often (`hits` = DevTools-lite). */
export interface XraySiteCandidate {
  host: string;
  hits: number;
  strong: boolean;
}

export interface XrayConfigView {
  subscription_url: string;
  selected: number;
  socks_port: number;
  http_port: number;
  autostart: boolean;
  /** Whitelist: only these sites go through the tunnel. */
  proxy_sites: string[];
  /** Windows system proxy points at the local HTTP inbound. */
  system_proxy: boolean;
  installed_version: string;
  /** Unix time of the last domain-list update, 0 when never updated. */
  bases_updated_at: number;
  /** Switch to the fastest live location when the current one stops working. */
  auto_switch: boolean;
}

/** A long download the UI should show progress for. */
export interface DownloadProgress {
  id: string;
  label: string;
  received: number;
  total: number;
  done: boolean;
  error: string | null;
}

/** A ready-made service: one `geosite:` category behind a human name. */
export interface XraySiteBundle {
  name: string;
  category: string;
  domains: number;
  available: boolean;
  /** Traffic the proxy cannot carry, e.g. Discord voice over UDP. */
  note: string | null;
}

export interface XrayBasesSummary {
  categories: number;
  domains: number;
  updated_at: number;
}

export interface TgwsNotice {
  ok: boolean;
  message: string;
}

export interface TgwsUpdateInfo {
  installed: string;
  latest: string | null;
  latest_tag: string | null;
  has_update: boolean;
  asset_name: string | null;
  asset_url: string | null;
  asset_size: number;
  digest: string | null;
  release_url: string | null;
  published_at: string | null;
  dismissed: boolean;
}

export interface AppUpdateInfo {
  current: string;
  latest: string;
  available: boolean;
  url: string;
  notes: string;
  size: number;
  can_auto_update: boolean;
}

export interface UpdateInfo {
  installed: string | null;
  latest: string;
  update_available: boolean;
  changelog: string;
  download_size: number;
}

export interface WhatsNew {
  version: string;
  notes: string;
  url: string;
}

export type HostStatus = "ok" | "ssl" | "reset" | "timeout" | "dns" | "unsupported" | "error";

export type HostGroup = "discord" | "youtube" | "control";

export type Verdict = "works" | "partial" | "broken" | "disqualified";

export type BaselineGate = "notblocked" | "blocked" | "unmeasurable";

export interface HostProbe {
  name: string;
  url: string;
  group: HostGroup;
  status: HostStatus;
  ms: number;
  detail: string;
}

export interface CandidateResult {
  name: string;
  source: "official" | "generated";
  parent: string | null;
  score: number;
  verdict: Verdict;
  discord_ok: number;
  discord_total: number;
  youtube_ok: number;
  youtube_total: number;
  control_ok: number;
  control_total: number;
  avg_ms: number;
  error: string | null;
  hosts: HostProbe[];
}

export interface BenchSummary {
  results: CandidateResult[];
  baseline: HostProbe[];
  gate: BaselineGate;
  cancelled: boolean;
  best: string | null;
  applied: string | null;
  message: string | null;
  ran_at: number;
}

export interface BenchProgress {
  phase: string;
  index: number;
  total: number;
  name: string;
  percent: number;
  message: string;
}

export interface QuickCheck {
  running: boolean;
  score: number;
  verdict: Verdict;
  gate: BaselineGate;
  hosts: HostProbe[];
}
