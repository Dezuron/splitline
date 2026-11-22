import { computed, ref } from "vue";
import type {
  AppStatus,
  AppUpdateInfo,
  BenchProgress,
  BenchSummary,
  CandidateResult,
  ConfigView,
  DownloadProgress,
  QuickCheck,
  TgwsStatus,
  TgwsDiagnosis,
  TgwsUpdateInfo,
  XrayProfile,
  XrayStatus,
  UpdateInfo,
  WhatsNew,
} from "../types";
import { formatTimestamp } from "../lib/labels";
import { tgwsStateLabel } from "../lib/tgws";

export type SortMode = "name" | "score" | "latency";

export const status = ref<AppStatus | null>(null);
export const config = ref<ConfigView | null>(null);
export const updateInfo = ref<UpdateInfo | null>(null);
export const checkingUpdate = ref(false);
export const updateProgress = ref<string | null>(null);
export const appUpdate = ref<AppUpdateInfo | null>(null);
export const checkingAppUpdate = ref(false);
export const appUpdateProgress = ref<string | null>(null);
export const wizardDismissed = ref(false);
export const whatsNew = ref<WhatsNew | null>(null);
export const busy = ref(false);
export const selected = ref("");
export const starting = ref(false);
/** True while a settings archive is being written or read. */
export const backupBusy = ref(false);

export const benchSummary = ref<BenchSummary | null>(null);
export const benchProgress = ref<BenchProgress | null>(null);
export const benchRunning = ref(false);
export const benchBusy = ref(false);
export const quick = ref<QuickCheck | null>(null);
export const includeGenerated = ref(false);
export const budget = ref(40);
export const sortMode = ref<SortMode>("score");

export const tgwsStatus = ref<TgwsStatus | null>(null);
export const tgwsUpdate = ref<TgwsUpdateInfo | null>(null);
export const tgwsLogs = ref<string[]>([]);
export const tgwsLink = ref("");
export const tgwsBusy = ref(false);
export const tgwsChecking = ref(false);
export const tgwsProgress = ref<string | null>(null);

export const xrayStatus = ref<XrayStatus | null>(null);
export const xrayProfiles = ref<XrayProfile[]>([]);
export const xrayLogs = ref<string[]>([]);

/** Live tunnel traffic: bytes per second and cumulative session counters. */
export interface TrafficSample {
  up: number;
  down: number;
}
export const trafficUp = ref(0);
export const trafficDown = ref(0);
export const trafficTotalUp = ref(0);
export const trafficTotalDown = ref(0);
export const trafficSamples = ref<TrafficSample[]>([]);

/** The last 20 strategy runs, oldest first: the history sparkline. */
export const benchHistory = ref<BenchSummary[]>([]);

/** Result of the on-demand Telegram proxy check. */
export const tgwsDiagnosis = ref<TgwsDiagnosis | null>(null);
export const tgwsDiagnosing = ref(false);

/** Tunnel health: one request through SOCKS per minute while the VPN is up. */
export const tunnelOk = ref(false);
export const tunnelPing = ref(0);
export const tunnelSamples = ref<boolean[]>([]);
export const tunnelErrors = computed(() => tunnelSamples.value.filter((ok) => !ok).length);
export const tunnelSlo = computed(() => {
  if (!tunnelSamples.value.length) return 100;
  const ok = tunnelSamples.value.filter(Boolean).length;
  return Math.round((ok / tunnelSamples.value.length) * 100);
});

/** Long downloads in flight; rendered by DownloadOverlay.vue. */
export const downloads = ref<DownloadProgress[]>([]);
export const xrayBusy = ref(false);
export const xrayLink = ref("");
export const xrayError = ref<string | null>(null);

export const running = computed(() => status.value?.running ?? false);
export const runtimeReady = computed(() => status.value?.runtime_valid ?? false);
export const runtimePath = computed(() => config.value?.runtime_path ?? "");
export const tgwsRunning = computed(() => tgwsStatus.value?.state === "running");
export const tgwsHasUpdate = computed(
  () => !!tgwsUpdate.value?.has_update && !tgwsUpdate.value.dismissed,
);
export const showSetupWizard = computed(
  () => !!status.value && !runtimeReady.value && !wizardDismissed.value,
);
export const showWhatsNew = computed(() => !!whatsNew.value && !showSetupWizard.value);
export const currentName = computed(() => status.value?.current_strategy ?? "");
export const appVersion = computed(() => status.value?.app_version ?? "");

export const vpnActive = computed(() => xrayStatus.value?.state === "running");

/** Activity is only claimed when a client or traffic event happened recently. */
export const ACTIVE_WINDOW_SEC = 5 * 60;

/** Backend timestamps are Unix seconds; a millisecond value is tolerated. */
function toSeconds(timestamp: number): number {
  return timestamp > 1_000_000_000_000 ? Math.round(timestamp / 1000) : timestamp;
}

export function secondsAgo(timestamp: number | null | undefined, now = Date.now()): number | null {
  if (!timestamp || timestamp <= 0) return null;
  return Math.max(0, Math.round(now / 1000 - toSeconds(timestamp)));
}

export function isRecentlyActive(
  timestamp: number | null | undefined,
  now = Date.now(),
  windowSec = ACTIVE_WINDOW_SEC,
): boolean {
  const ago = secondsAgo(timestamp, now);
  return ago !== null && ago <= windowSec;
}

/** "12 с назад", "3 мин назад": one wording for every screen. */
export function formatAgo(timestamp: number | null | undefined, now = Date.now()): string {
  const ago = secondsAgo(timestamp, now);
  if (ago === null) return "—";
  if (ago < 5) return "только что";
  if (ago < 60) return `${ago} с назад`;
  const minutes = Math.floor(ago / 60);
  if (minutes < 60) return `${minutes} мин назад`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} ч назад`;
  return `${Math.floor(hours / 24)} дн назад`;
}

export function plural(count: number, one: string, few: string, many: string): string {
  const mod10 = count % 10;
  const mod100 = count % 100;
  if (mod10 === 1 && mod100 !== 11) return one;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return few;
  return many;
}

/** `unknown` means the backend does not report activity yet — never guessed. */
export type ActivityState = "active" | "idle" | "unknown";

/**
 * Telegram activity comes from the proxy's live `active=` counter. A proxy
 * that reports neither `active_connections` nor `last_client_at` is `unknown`:
 * the cumulative `client_connections_total` is diagnostics only, because it
 * says "a client connected once", not "a client is connected now".
 */
export const tgwsActivity = computed<ActivityState>(() => {
  if (!tgwsRunning.value) return "idle";
  const snapshot = tgwsStatus.value;
  if (!snapshot) return "unknown";
  if (typeof snapshot.active_connections === "number") {
    return snapshot.active_connections > 0 ? "active" : "idle";
  }
  if (isRecentlyActive(snapshot.last_client_at)) return "active";
  return snapshot.last_client_at ? "idle" : "unknown";
});

export const tgwsActiveConnections = computed<number | null>(() => {
  if (!tgwsRunning.value) return 0;
  const snapshot = tgwsStatus.value;
  if (!snapshot) return null;
  if (typeof snapshot.active_connections === "number") return snapshot.active_connections;
  if (snapshot.last_client_at) return isRecentlyActive(snapshot.last_client_at) ? 1 : 0;
  return null;
});

/** "—" or "12 с назад": the last client event, for the cards and the page. */
export const tgwsLastActivityLabel = computed(() => formatAgo(tgwsStatus.value?.last_client_at));

/** The page/card line: "Прокси включён · Telegram не подключён" and friends. */
export const tgwsStatusText = computed(() => {
  if (!tgwsStatus.value) return "Загрузка…";
  if (!tgwsRunning.value) return tgwsStateLabel(tgwsStatus.value.state);
  if (tgwsActivity.value === "active") {
    const count = tgwsActiveConnections.value ?? 0;
    return `Telegram работает через прокси · ${count} ${plural(count, "соединение", "соединения", "соединений")}`;
  }
  if (tgwsActivity.value === "idle") return "Прокси включён · Telegram не подключён";
  return "Прокси включён";
});

/** Traffic seen by the poller: `get_xray_traffic` deltas are the only source. */
export const xrayLastTrafficAt = ref(0);

export const vpnActivity = computed<ActivityState>(() => {
  if (!xrayStatus.value || !vpnActive.value) return "idle";
  return isRecentlyActive(xrayLastTrafficAt.value) ? "active" : "idle";
});

/** "VPN работает" only with traffic; a live core alone stays neutral. */
export const vpnStatusText = computed(() => {
  if (!xrayStatus.value) return "Загрузка…";
  switch (xrayStatus.value.state) {
    case "running":
      return vpnActivity.value === "active" ? "VPN работает" : "VPN включён · трафика нет";
    case "starting":
      return "Подключение…";
    case "stopping":
      return "Отключение…";
    case "error":
      return "Ошибка";
    case "not_installed":
      return "Ядро не установлено";
    default:
      return "VPN выключен";
  }
});

export const zapretStatusText = computed(() => {
  if (!status.value) return "Загрузка…";
  if (benchRunning.value) return "Идёт проверка стратегий";
  if (!runtimeReady.value) return "flowseal-zapret не установлен";
  return running.value ? "Обход включён" : "Обход выключен";
});

/** The Zapret page's dot only reacts to the bypass, never to Telegram or VPN. */
export const zapretTone = computed<"ok" | "idle" | "danger">(() => {
  if (!status.value || benchRunning.value) return "idle";
  if (!runtimeReady.value) return "danger";
  return running.value ? "ok" : "idle";
});

export interface ServiceStatus {
  name: string;
  on: boolean;
  state: ActivityState;
}

/** One source of truth for the chip, the cards and the pages. */
export const servicesStatus = computed<ServiceStatus[]>(() => {
  const zapretOn = runtimeReady.value && running.value;
  return [
    { name: "Zapret", on: zapretOn, state: zapretOn ? "active" : "idle" },
    { name: "Telegram", on: tgwsRunning.value, state: tgwsActivity.value },
    { name: "VPN", on: vpnActive.value, state: vpnActivity.value },
  ];
});

export const activeServiceCount = computed(
  () => servicesStatus.value.filter((service) => service.state === "active").length,
);

/** The status line reads "A, B и C" — names in the order they matter. */
function joinActive(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} и ${names[names.length - 1]}`;
}

function idlePhrase(service: ServiceStatus): string {
  if (service.name === "Telegram") {
    return service.state === "unknown" ? "Telegram включён" : "Telegram не подключён";
  }
  return service.state === "unknown" ? "VPN включён" : "VPN без трафика";
}

export const statusText = computed(() => {
  if (!status.value) return "Загрузка…";
  if (benchRunning.value) return "Идёт проверка стратегий";

  const on = servicesStatus.value.filter((service) => service.on);
  const active = on.filter((service) => service.state === "active");

  if (!on.length) {
    return status.value.runtime_valid ? "Все сервисы выключены" : "flowseal-zapret не установлен";
  }

  if (active.length === on.length) {
    const names = active.map((service) => service.name);
    return `${joinActive(names)} ${names.length > 1 ? "работают" : "работает"}`;
  }

  if (!active.length) {
    if (on.length === 1) {
      const [only] = on;
      if (only.name === "Telegram") {
        return only.state === "unknown"
          ? "Прокси включён"
          : "Прокси включён · Telegram не подключён";
      }
      return "VPN включён · трафика нет";
    }
    return `${joinActive(on.map((service) => service.name))} включены`;
  }

  const names = active.map((service) => service.name);
  const idle = on.filter((service) => service.state !== "active").map(idlePhrase);
  return `${joinActive(names)} ${names.length > 1 ? "работают" : "работает"} · ${idle.join(" · ")}`;
});

export const statusTone = computed<"ok" | "idle" | "danger">(() => {
  if (!status.value) return "idle";
  if (benchRunning.value) return "idle";

  const services = servicesStatus.value;
  if (services.some((service) => service.state === "active")) return "ok";
  if (!status.value.runtime_valid && !services.some((service) => service.on)) return "danger";
  return "idle";
});

const benchByName = computed(() => {
  const map = new Map<string, CandidateResult>();
  for (const result of benchSummary.value?.results ?? []) {
    map.set(result.name, result);
  }
  return map;
});

function sortRows<T extends { name: string }>(rows: T[]): T[] {
  if (sortMode.value === "name") return rows;
  const collator = new Intl.Collator("ru");
  return [...rows].sort((a, b) => {
    const left = benchByName.value.get(a.name);
    const right = benchByName.value.get(b.name);
    if (sortMode.value === "score") {
      const leftScore = left?.score ?? -1;
      const rightScore = right?.score ?? -1;
      if (leftScore !== rightScore) return rightScore - leftScore;
    } else {
      const leftMs = left && left.avg_ms > 0 ? left.avg_ms : Number.MAX_SAFE_INTEGER;
      const rightMs = right && right.avg_ms > 0 ? right.avg_ms : Number.MAX_SAFE_INTEGER;
      if (leftMs !== rightMs) return leftMs - rightMs;
    }
    return collator.compare(a.name, b.name);
  });
}

export interface StrategyRowData {
  name: string;
  active: boolean;
  args_preview: string;
  generated: boolean;
  parent?: string;
  bench?: CandidateResult;
}

const officialAll = computed<StrategyRowData[]>(() =>
  (status.value?.strategies ?? []).map((strategy) => ({
    ...strategy,
    generated: false,
    bench: benchByName.value.get(strategy.name),
  })),
);

const generatedAll = computed<StrategyRowData[]>(() =>
  (status.value?.generated ?? []).map((strategy) => ({
    ...strategy,
    generated: true,
    bench: benchByName.value.get(strategy.name),
  })),
);

// The applied strategy, rendered as the first row of the list. It is kept out
// of its own group so the list never shows it twice.
export const activeRow = computed<StrategyRowData | null>(
  () => [...officialAll.value, ...generatedAll.value].find((row) => row.active) ?? null,
);

export const officialRows = computed<StrategyRowData[]>(() =>
  sortRows(officialAll.value.filter((row) => !row.active)),
);

export const generatedRows = computed<StrategyRowData[]>(() =>
  sortRows(generatedAll.value.filter((row) => !row.active)),
);

export const benchDate = computed(() =>
  benchSummary.value ? formatTimestamp(benchSummary.value.ran_at) : "",
);

export const benchHostCount = computed(() => benchSummary.value?.baseline.length ?? 0);

export const gateMessage = computed(() => {
  const gate = benchSummary.value?.gate;
  if (!gate) return "";
  if (gate === "notblocked")
    return "Без обхода цели доступны — DPI не блокирует Discord/YouTube, перебор не нужен.";
  if (gate === "unmeasurable") return "Не удалось замерить цели без обхода: проверьте DNS и сеть.";
  return "";
});
