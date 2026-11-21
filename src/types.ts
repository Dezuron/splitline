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

/**
 * How autostart ended up registered. `run-key` means the Task Scheduler task
 * could not be created and Windows will ask for confirmation at logon.
 */
export interface AutostartStatus {
  mode: "task" | "run-key" | "disabled";
  warning: string | null;
}

export interface TgwsConfigView {
  autostart: boolean;
  host: string;
  port: number;
  secret: string;
  installed_version: string;
  /**
   * Engine the config points at: `python` (proven build) or `rs` (light beta).
   * Optional because a build without the second engine does not report it.
   */
  engine?: string;
  /**
   * Cloudflare domains in the user's priority order. `undefined` means the
   * backend does not report the list, and the page must not touch it.
   */
  cf_domains?: string[];
  /** Cloudflare Worker domain for the TCP fallback; empty means not configured. */
  cf_worker_domain?: string;
  /** Rotate across the domain list instead of always starting from the first. */
  cf_balance?: boolean;
  /** Inbound FakeTLS domain; empty keeps the link in the plain `dd` form. */
  fake_tls_domain?: string;
}

export type TgwsState = "stopped" | "starting" | "running" | "stopping" | "error" | "not_installed";

export interface TgwsStatus {
  state: TgwsState;
  pid: number | null;
  last_error: string | null;
  installed_version: string;
  binary_present: boolean;
  port_conflict: boolean;
  /**
   * Total connections accepted since the proxy started. Diagnostics only: it
   * never resets, so it cannot answer "is a client connected now".
   */
  client_connections_total: number;
  /** Clients connected right now, parsed from the proxy's `active=`. */
  active_connections?: number | null;
  /** Unix seconds of the last client event; null while the proxy has none. */
  last_client_at?: number | null;
  /** Unix seconds of the last forwarded line; idle clients leave it behind. */
  last_traffic_at?: number | null;
  /** 1-based number of the current automatic restart, 0 when not restarting. */
  restart_attempt?: number;
  /** Engine the config points at: `python` or `rs`. */
  engine?: string;
  /** Consecutive failed starts of the new engine; the old one ignores it. */
  engine_start_failures?: number;
  /** The new engine failed three times in a row: the page offers the old one. */
  fallback_recommended?: boolean;
  /** The new engine's binary is on disk, so switching needs no download. */
  rust_binary_present?: boolean;
}

/** One run of the new engine's `--check-listener` self-check. */
export interface TgwsListenerCheck {
  /** Whether the probe actually ran. A running proxy makes it "not now". */
  ran: boolean;
  ok: boolean;
  /** One sentence for the card. */
  summary: string;
  /** The probe's own last line, with the secret masked. */
  detail: string | null;
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
  kind: "ok" | "binary" | "port" | "stopped" | "not_connected" | "blocked";
  /** Engine the result belongs to: `python` or `rs`. */
  engine?: string;
  /** The new engine's own probe; absent for the old engine, which has none. */
  listener?: TgwsListenerCheck | null;
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

/** A file inside a settings archive, as the import dialog lists it. */
export interface BackupFile {
  path: string;
  size: number;
}

/** Verified preview of an archive; nothing has been written yet. */
export interface BackupPreview {
  schema: number;
  app: string;
  app_version: string;
  created_at: number;
  created_label: string;
  /** The archive carries the proxy key and the subscription URL. */
  secrets: boolean;
  files: BackupFile[];
  has_config: boolean;
  lists: number;
  generated: number;
  bench_runs: number;
  has_subscription: boolean;
  /** One line for the confirmation dialog. */
  summary: string;
  /** What the user must know before confirming. */
  warnings: string[];
}

export interface BackupExportReport {
  path: string;
  files: string[];
  secrets: boolean;
  size: number;
}

export interface BackupImportReport {
  applied: string[];
  lists: number;
  /** Safety copy of the state that was on disk before the import. */
  backup_path: string;
  warnings: string[];
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

/** What the tray wants to tell the user; `visible` says whether the window was open. */
export interface TrayNotice {
  message: string;
  visible: boolean;
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
