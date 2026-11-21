import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import type {
  AppStatus,
  AppUpdateInfo,
  AutostartStatus,
  BackupExportReport,
  BackupImportReport,
  BackupPreview,
  BenchProgress,
  BenchSummary,
  ConfigView,
  DownloadProgress,
  QuickCheck,
  TgwsNotice,
  TgwsStatus,
  TgwsDiagnosis,
  TgwsUpdateInfo,
  TrayNotice,
  UpdateInfo,
  WhatsNew,
  XrayBasesSummary,
  XrayProfile,
  XraySiteBundle,
  XraySiteCandidate,
  XrayStatus,
} from "../types";
import { confirmAction } from "../lib/confirm";
import { backupFileName, fileNameOf } from "../lib/backup";
import { toast } from "../lib/toast";
import { tgwsLogTail } from "../lib/tgws";
import {
  flushTrayToasts,
  queueTrayToast,
  showTrayNotice,
  syncTray,
  trayHintOnce,
} from "../lib/tray";
import {
  status,
  config,
  updateInfo,
  checkingUpdate,
  updateProgress,
  appUpdate,
  checkingAppUpdate,
  appUpdateProgress,
  wizardDismissed,
  whatsNew,
  busy,
  selected,
  starting,
  backupBusy,
  benchSummary,
  benchProgress,
  benchRunning,
  benchBusy,
  quick,
  budget,
  running,
  currentName,
  generatedRows,
  tgwsStatus,
  tgwsUpdate,
  tgwsDiagnosis,
  tgwsDiagnosing,
  benchHistory,
  trafficDown,
  trafficSamples,
  trafficTotalDown,
  trafficTotalUp,
  trafficUp,
  tunnelOk,
  tunnelPing,
  tunnelSamples,
  tgwsLogs,
  tgwsLink,
  tgwsBusy,
  tgwsChecking,
  tgwsProgress,
  xrayStatus,
  xrayProfiles,
  xrayLastTrafficAt,
  xrayLogs,
  downloads,
  xrayBusy,
  xrayLink,
  xrayError,
} from "./state";

// ---------- polling ----------

let pollTimer: number | null = null;
let trafficTimer: number | null = null;
let pingTimer: number | null = null;
let probeTimer: number | null = null;
let refreshTimer: number | null = null;
let tunnelFailures = 0;
let manualPickAt = 0;
let lastTrafficUp = 0;
let lastTrafficDown = 0;
const TRAFFIC_INTERVAL_MS = 2000;
let appUpdateTimer: number | null = null;
const downloadTimers = new Map<string, number>();
let lastRefreshError = 0;
let refreshSeq = 0;
let unlisteners: Array<() => void> = [];
let initialized = false;
let budgetInitialized = false;
let windowShown = false;

export const THEMES = ["satellite", "carmine"] as const;
export type ThemeId = (typeof THEMES)[number];

/** Themes from earlier builds: every dark variant collapses into satellite,
 * and the old paper theme maps to carmine. */
const LEGACY_THEMES: Record<string, ThemeId> = {
  dark: "satellite",
  gray: "satellite",
  orbital: "satellite",
  orbit: "satellite",
  stencil: "carmine",
  light: "carmine",
};

export function isThemeId(value: string | undefined): value is ThemeId {
  return !!value && (THEMES as readonly string[]).includes(value);
}

export function normalizeTheme(value: string | undefined): ThemeId {
  if (isThemeId(value)) return value;
  return (value && LEGACY_THEMES[value]) || "satellite";
}

/** Where the applied theme is cached: read in `main.ts` before the first render. */
export const THEME_CACHE_KEY = "splitline.theme";

export function readCachedTheme(): ThemeId | undefined {
  try {
    const value = localStorage.getItem(THEME_CACHE_KEY);
    return value ? normalizeTheme(value) : undefined;
  } catch {
    // Storage can be unavailable; the caller falls back to the default theme.
    return undefined;
  }
}

export function applyTheme(theme: string | undefined) {
  const normalized = normalizeTheme(theme);
  document.documentElement.dataset.theme = normalized;
  // Both remaining themes are dark; keep Tailwind/shadcn `dark:` variants on.
  document.documentElement.classList.add("dark");
  try {
    // `refreshInternal` reapplies the theme every few seconds: skip the write
    // when the cache already holds it.
    if (localStorage.getItem(THEME_CACHE_KEY) !== normalized) {
      localStorage.setItem(THEME_CACHE_KEY, normalized);
    }
  } catch {
    // The theme still applies; only the next cold start loses the cache.
  }
}

/**
 * Shows the window once the configured theme has been painted. The Rust side
 * keeps the window hidden until this call, so no frame of the default theme is
 * visible; `Promise.resolve` keeps mocked `invoke` (tests) from throwing.
 */
function revealWindow() {
  if (windowShown) return;
  windowShown = true;
  void Promise.resolve(invoke("frontend_ready")).catch(() => undefined);
}

async function refreshInternal(quiet: boolean) {
  // Allow overlapping calls (a polling tick may coincide with an action) but
  // only let the latest one write state, so responses cannot land out of order.
  const seq = ++refreshSeq;
  try {
    const nextStatus = await invoke<AppStatus>("get_state");
    if (seq !== refreshSeq) return;
    const nextConfig = await invoke<ConfigView>("get_config");
    if (seq !== refreshSeq) return;
    status.value = nextStatus;
    config.value = nextConfig;
    applyTheme(nextConfig.theme);
    if (!windowShown) {
      // Two frames make sure the new theme has been painted before the window
      // appears; the timer covers a renderer that throttles animation frames
      // while the window is hidden.
      const raf =
        typeof window.requestAnimationFrame === "function"
          ? window.requestAnimationFrame.bind(window)
          : (callback: () => void) => window.setTimeout(callback, 16);
      raf(() => raf(revealWindow));
      window.setTimeout(revealWindow, 250);
    }
    if (!budgetInitialized && config.value) {
      budget.value = config.value.bench_budget;
      budgetInitialized = true;
    }
    const exists =
      nextStatus.strategies.some((s) => s.name === selected.value) ||
      nextStatus.generated.some((g) => g.name === selected.value);
    if (!exists) selected.value = nextStatus.current_strategy || "";
    syncTrayState();
  } catch (error) {
    const now = Date.now();
    if (!quiet || now - lastRefreshError > 30_000) {
      lastRefreshError = now;
      toast("error", `Не удалось обновить состояние: ${error}`);
    }
  }
}

/**
 * Mirrors the running services into the tray. Texts and "installed" checks stay
 * in Rust; zapret is read there too, so this call also picks up a freshly
 * installed runtime or subscription.
 */
function syncTrayState() {
  void syncTray({
    tgws: tgwsStatus.value?.state === "running",
    xray: xrayStatus.value?.state === "running",
  });
}

export function refresh(quiet = true) {
  return refreshInternal(quiet);
}

function startPolling() {
  if (pollTimer !== null) return;
  pollTimer = window.setInterval(() => {
    // Do not spawn sc/tasklist while hidden in the tray or during a benchmark.
    if (!document.hidden && !benchRunning.value) {
      void refreshInternal(true);
      // The VPN banner depends on live SOCKS connections, so the status is
      // polled too: no event fires when Telegram merely connects.
      if (xrayStatus.value?.state === "running") {
        void refreshXray();
      }
    }
  }, 4000);
}

function stopPolling() {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/** Reads the core's cumulative counters and turns the deltas into speeds. */
async function pollTraffic() {
  if (document.hidden) return;
  if (xrayStatus.value?.state !== "running") {
    if (trafficSamples.value.length) {
      trafficSamples.value = [];
      trafficUp.value = 0;
      trafficDown.value = 0;
      trafficTotalUp.value = 0;
      trafficTotalDown.value = 0;
      lastTrafficUp = 0;
      lastTrafficDown = 0;
    }
    xrayLastTrafficAt.value = 0;
    return;
  }
  try {
    const stats = await invoke<{ up: number; down: number }>("get_xray_traffic");
    // Counters restart with the core; a smaller value means "since restart".
    const deltaUp = stats.up >= lastTrafficUp ? stats.up - lastTrafficUp : stats.up;
    const deltaDown = stats.down >= lastTrafficDown ? stats.down - lastTrafficDown : stats.down;
    lastTrafficUp = stats.up;
    lastTrafficDown = stats.down;
    trafficTotalUp.value = stats.up;
    trafficTotalDown.value = stats.down;
    // The green "VPN работает" state needs a recent byte, not just a live core.
    if (deltaUp > 0 || deltaDown > 0) {
      xrayLastTrafficAt.value = Math.floor(Date.now() / 1000);
    }
    const seconds = TRAFFIC_INTERVAL_MS / 1000;
    trafficUp.value = deltaUp / seconds;
    trafficDown.value = deltaDown / seconds;
    trafficSamples.value = [
      ...trafficSamples.value.slice(-(150 - 1)),
      { up: trafficUp.value, down: trafficDown.value },
    ];
  } catch {
    // The core may still be starting; keep the previous sample.
  }
}

function startTrafficPolling() {
  if (trafficTimer !== null) return;
  void pollTraffic();
  trafficTimer = window.setInterval(pollTraffic, TRAFFIC_INTERVAL_MS);
}

function stopTrafficPolling() {
  if (trafficTimer !== null) {
    clearInterval(trafficTimer);
    trafficTimer = null;
  }
}

/** Refreshes every location's latency in parallel and measures the tunnel. */
function startPingTimer() {
  if (pingTimer !== null) return;
  void sweepPings();
  pingTimer = window.setInterval(() => {
    if (!document.hidden) void sweepPings();
  }, 5 * 60_000);
}

async function sweepPings() {
  try {
    xrayProfiles.value = await invoke<XrayProfile[]>("get_xray_profiles", { withPing: true });
  } catch {
    // The subscription may not be loaded yet; the next sweep will retry.
  }
}

function stopPingTimer() {
  if (pingTimer !== null) {
    clearInterval(pingTimer);
    pingTimer = null;
  }
}

/** One request through the tunnel per minute: availability, SLO and failover. */
async function probeTunnel() {
  if (document.hidden) return;
  if (xrayStatus.value?.state !== "running") return;
  try {
    const probe = await invoke<{ ok: boolean; ms: number }>("get_xray_tunnel_probe");
    tunnelOk.value = probe.ok;
    if (probe.ok) tunnelPing.value = probe.ms;
    tunnelSamples.value = [...tunnelSamples.value.slice(-(60 - 1)), probe.ok];
    if (probe.ok) {
      tunnelFailures = 0;
      return;
    }
    tunnelFailures += 1;
    // Five failed minutes in a row: switch, but never right after a manual pick.
    if (
      tunnelFailures >= 5 &&
      config.value?.xray.auto_switch &&
      Date.now() - manualPickAt > 5 * 60_000
    ) {
      tunnelFailures = 0;
      const best = [...xrayProfiles.value]
        .filter(
          (profile) => profile.ping_ms != null && profile.index !== xrayStatus.value?.selected,
        )
        .sort((left, right) => (left.ping_ms ?? 0) - (right.ping_ms ?? 0))[0];
      if (best) {
        toast("warning", `Локация не отвечает — переключаюсь на «${best.name}».`);
        await selectXrayProfile(best.index);
      } else {
        toast("warning", "Нет доступных локаций — проверьте сеть или подписку.");
      }
    }
  } catch {
    // A failed probe is expected while the core is starting; the next tick retries.
  }
}

function startProbeTimer() {
  if (probeTimer !== null) return;
  void probeTunnel();
  probeTimer = window.setInterval(probeTunnel, 60_000);
}

function stopProbeTimer() {
  if (probeTimer !== null) {
    clearInterval(probeTimer);
    probeTimer = null;
  }
}

/** Refreshes the subscription in the background when it is older than 12 hours. */
function startRefreshTimer() {
  if (refreshTimer !== null) return;
  refreshTimer = window.setInterval(() => {
    if (document.hidden) return;
    const current = xrayStatus.value;
    const url = config.value?.xray.subscription_url;
    if (!current || !url || xrayBusy.value) return;
    if (Date.now() / 1000 - current.fetched_at < 12 * 3600) return;
    void refreshXraySubscription(false).catch(() => undefined);
  }, 3600_000);
}

function stopRefreshTimer() {
  if (refreshTimer !== null) {
    clearInterval(refreshTimer);
    refreshTimer = null;
  }
}

function onVisibilityChange() {
  if (document.hidden) {
    stopPolling();
    stopTrafficPolling();
    stopPingTimer();
    stopProbeTimer();
    stopRefreshTimer();
  } else {
    void refreshInternal(true);
    startPolling();
    startTrafficPolling();
    startPingTimer();
    startProbeTimer();
    startRefreshTimer();
    flushTrayToasts();
  }
}

export async function initStore() {
  if (initialized) return;
  initialized = true;

  unlisteners = [
    await listen<string>("update-progress", (event) => {
      updateProgress.value = event.payload;
    }),
    await listen<BenchProgress>("bench-progress", (event) => {
      // `benchRunning` is owned by runBenchCommand's lifecycle; progress events
      // must not re-arm it after the command has already finished.
      benchProgress.value = event.payload;
    }),
    await listen<string>("auto-rollback", async (event) => {
      toast("warning", event.payload);
      await refreshInternal(true);
    }),
    await listen<string>("bench-warning", (event) => {
      toast("warning", event.payload);
    }),
    await listen<string>("app-update-progress", (event) => {
      appUpdateProgress.value = event.payload;
    }),
    await listen<TgwsStatus>("tgws-status", (event) => {
      tgwsStatus.value = event.payload;
      syncTrayState();
    }),
    await listen<string>("tgws-log", (event) => {
      tgwsLogs.value = tgwsLogTail([...tgwsLogs.value, event.payload]);
    }),
    await listen<string>("tgws-progress", (event) => {
      tgwsProgress.value = event.payload || null;
    }),
    await listen<TgwsNotice>("tgws-notice", async (event) => {
      toast(event.payload.ok ? "success" : "warning", event.payload.message);
      await refreshTgws();
    }),
    await listen<XrayStatus>("xray-status", (event) => {
      xrayStatus.value = event.payload;
      syncTrayState();
    }),
    await listen<string>("proxy-guard", (event) => {
      toast("warning", event.payload);
      void refreshXray();
    }),
    await listen<string>("proxy-guard-restored", (event) => {
      toast("info", event.payload);
      void refreshXray();
    }),
    await listen<string>("xray-log", (event) => {
      xrayLogs.value = tgwsLogTail([...xrayLogs.value, event.payload]);
    }),
    await listen<DownloadProgress>("download-progress", (event) => {
      applyDownloadProgress(event.payload);
    }),
    // A failure from the tray must reach the user: now if the window is open,
    // otherwise on the next show — a toast behind a hidden window is wasted.
    await listen<TrayNotice>("tray-notice", (event) => {
      showTrayNotice(event.payload);
    }),
    await listen("app-hidden", () => {
      // Explained once: the window is gone from the taskbar but the app lives.
      const hint = trayHintOnce();
      if (hint) queueTrayToast("info", hint);
    }),
    await listen("app-shown", () => {
      flushTrayToasts();
      // The tray can start or stop a service while the window is hidden: pick
      // the state up at once instead of waiting for the next poll tick.
      void refreshInternal(true);
      void refreshTgws();
      void refreshXray();
    }),
  ];

  await refreshInternal(false);
  void refreshTgws();
  void refreshXray();
  // Silent proxy update check on startup, like the app and zapret checks.
  // The backend cache keeps this to one GitHub request per 12 hours.
  void checkTgwsUpdate(true);
  await loadWhatsNew();
  if (config.value?.theme) {
    void invoke("set_theme", { theme: config.value.theme }).catch(() => undefined);
  }
  try {
    benchSummary.value = await invoke<BenchSummary | null>("get_last_bench");
  } catch (error) {
    console.warn("Не удалось загрузить историю проверок", error);
  }
  try {
    benchHistory.value = await invoke<BenchSummary[]>("get_bench_history");
  } catch (error) {
    console.warn("Не удалось загрузить историю прогонов", error);
  }
  if (config.value?.check_updates) {
    void checkUpdate();
  }
  if (config.value?.check_app_updates) {
    appUpdateTimer = window.setTimeout(() => {
      void checkAppUpdate(true);
    }, 3000);
  }
  startPolling();
  startTrafficPolling();
  startPingTimer();
  startProbeTimer();
  startRefreshTimer();
  document.addEventListener("visibilitychange", onVisibilityChange);
}

/** Keeps the download overlay in sync; a finished entry fades out shortly. */
function applyDownloadProgress(progress: DownloadProgress) {
  const existing = downloads.value.find((item) => item.id === progress.id);
  if (progress.error) {
    if (existing) {
      existing.label = progress.label;
      existing.error = progress.error;
      existing.done = true;
    } else {
      downloads.value.push({ ...progress });
    }
    scheduleDownloadRemoval(progress.id, 6000);
    return;
  }
  if (progress.done) {
    if (existing) {
      existing.received = progress.received;
      existing.total = progress.total;
      existing.done = true;
    }
    scheduleDownloadRemoval(progress.id, 900);
    return;
  }
  if (existing) {
    existing.label = progress.label;
    existing.received = progress.received;
    existing.total = progress.total;
    existing.done = false;
    existing.error = null;
  } else {
    downloads.value.push({ ...progress });
  }
}

function scheduleDownloadRemoval(id: string, delay: number) {
  const previous = downloadTimers.get(id);
  if (previous !== undefined) clearTimeout(previous);
  const timer = window.setTimeout(() => {
    downloadTimers.delete(id);
    downloads.value = downloads.value.filter((item) => item.id !== id);
  }, delay);
  downloadTimers.set(id, timer);
}

export function disposeStore() {
  stopPolling();
  stopTrafficPolling();
  stopPingTimer();
  stopProbeTimer();
  stopRefreshTimer();
  if (appUpdateTimer !== null) {
    clearTimeout(appUpdateTimer);
    appUpdateTimer = null;
  }
  for (const timer of downloadTimers.values()) clearTimeout(timer);
  downloadTimers.clear();
  downloads.value = [];
  for (const unlisten of unlisteners) unlisten();
  unlisteners = [];
  document.removeEventListener("visibilitychange", onVisibilityChange);
  initialized = false;
}

// ---------- actions ----------

export async function toggleStart() {
  if (busy.value || benchRunning.value) return;
  const wasRunning = running.value;

  if (wasRunning) {
    const { ok } = await confirmAction({
      title: "Остановить обход?",
      text: "Служба zapret и драйвер WinDivert будут удалены. Discord и YouTube снова могут стать недоступны.",
      confirmLabel: "Остановить",
      danger: true,
    });
    if (!ok) return;
  }

  busy.value = true;
  starting.value = true;
  try {
    if (wasRunning) {
      await invoke("stop");
    } else {
      if (selected.value) {
        await invoke("set_strategy", { name: selected.value });
      }
      await invoke("start");
    }
    await refreshInternal(true);
    toast("success", wasRunning ? "Обход остановлен." : "Обход запущен.");
  } catch (error) {
    toast("error", String(error));
  } finally {
    busy.value = false;
    starting.value = false;
  }
}

export function selectStrategy(name: string) {
  if (benchRunning.value) return;
  selected.value = name;
}

export async function applyStrategy(name = selected.value) {
  if (!name || busy.value || benchRunning.value) return;
  if (name === currentName.value && running.value) {
    toast("info", "Эта стратегия уже активна.");
    return;
  }

  if (!running.value) {
    busy.value = true;
    try {
      await invoke("set_strategy", { name });
      await refreshInternal(true);
      toast("info", `Выбрана «${name}».`);
    } catch (error) {
      toast("error", String(error));
    } finally {
      busy.value = false;
    }
    return;
  }

  const { ok } = await confirmAction({
    title: "Перезапустить обход?",
    text: `Текущее соединение прервётся, обход перезапустится со стратегией «${name}».`,
    confirmLabel: "Перезапустить",
  });
  if (!ok) return;

  busy.value = true;
  try {
    await invoke("apply_candidate", { name });
    await refreshInternal(true);
    toast("success", `Применена «${name}».`);
  } catch (error) {
    toast("error", String(error));
  } finally {
    busy.value = false;
  }
}

export async function quickCheck() {
  if (benchBusy.value || benchRunning.value) return;
  benchBusy.value = true;
  quick.value = null;
  try {
    quick.value = await invoke<QuickCheck>("quick_check");
  } catch (error) {
    toast("error", String(error));
  } finally {
    benchBusy.value = false;
  }
}

async function runBenchCommand(
  command: "run_bench" | "run_search",
  args: Record<string, unknown>,
  doneLabel: string,
) {
  if (benchBusy.value) return;
  benchBusy.value = true;
  benchRunning.value = true;
  quick.value = null;
  try {
    const summary = await invoke<BenchSummary>(command, args);
    benchSummary.value = summary;
    if (summary.applied) {
      toast("success", `${doneLabel} Применена «${summary.applied}».`);
    } else if (summary.message) {
      toast("warning", summary.message);
    } else if (summary.cancelled) {
      toast("info", "Проверка отменена, результаты частичные.");
    }
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  } finally {
    benchBusy.value = false;
    benchRunning.value = false;
    benchProgress.value = null;
  }
}

export async function runBench(include: boolean) {
  if (benchBusy.value) return;
  const { ok, checked } = await confirmAction({
    title: "Проверить все стратегии?",
    text: "Обход будет остановлен на 2–5 минут, интернет станет нестабильным. После проверки служба поднимется снова.",
    confirmLabel: "Начать проверку",
    checkboxLabel: "Применить лучшую стратегию автоматически",
    checkboxValue: config.value?.bench_auto_apply ?? false,
  });
  if (!ok) return;
  await runBenchCommand(
    "run_bench",
    { includeGenerated: include, autoApply: checked },
    "Проверка завершена.",
  );
}

export async function runSearch() {
  if (benchBusy.value) return;
  const { ok, checked } = await confirmAction({
    title: "Подобрать новые стратегии?",
    text: "Сначала проверятся существующие, затем 20–100 мутаций. Это займёт 5–10 минут и прервёт обход: интернет будет нестабильным.",
    confirmLabel: "Начать подбор",
    checkboxLabel: "Применить лучшую стратегию автоматически",
    checkboxValue: config.value?.bench_auto_apply ?? false,
  });
  if (!ok) return;
  await runBenchCommand(
    "run_search",
    { budget: budget.value, autoApply: checked },
    "Подбор завершён.",
  );
}

export async function cancelBench() {
  try {
    await invoke("cancel_bench");
  } catch (error) {
    toast("error", String(error));
  }
}

export async function deleteGenerated(name: string) {
  if (benchRunning.value) return;
  const { ok } = await confirmAction({
    title: "Удалить экспериментальную стратегию?",
    text: `«${name}» будет удалена. Это действие нельзя отменить.`,
    confirmLabel: "Удалить",
    danger: true,
  });
  if (!ok) return;
  try {
    await invoke("delete_generated", { name });
    await refreshInternal(true);
    toast("success", `«${name}» удалена.`);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function deleteAllGenerated() {
  if (benchRunning.value) return;
  const count = generatedRows.value.length;
  if (!count) return;
  const { ok } = await confirmAction({
    title: "Удалить все экспериментальные стратегии?",
    text: `Будет удалено стратегий: ${count}. Активная стратегия останется, официальные не затрагиваются. Это действие нельзя отменить.`,
    confirmLabel: "Удалить все",
    danger: true,
  });
  if (!ok) return;
  try {
    const removed = await invoke<number>("delete_all_generated");
    await refreshInternal(true);
    toast("success", removed > 0 ? `Удалено стратегий: ${removed}.` : "Нечего удалять.");
  } catch (error) {
    toast("error", String(error));
  }
}

export async function checkUpdate(silent = true): Promise<boolean> {
  if (checkingUpdate.value) return false;
  checkingUpdate.value = true;
  try {
    updateInfo.value = await invoke<UpdateInfo>("check_update");
    if (updateInfo.value.update_available) {
      toast("info", `Доступно обновление flowseal-zapret ${updateInfo.value.latest}.`);
    } else if (!silent) {
      toast("info", "Обновлений flowseal-zapret нет.");
    }
    return true;
  } catch (error) {
    if (!silent) {
      toast("error", String(error));
    }
    return false;
  } finally {
    checkingUpdate.value = false;
  }
}

export async function checkAppUpdate(silent = true): Promise<boolean> {
  if (checkingAppUpdate.value) return false;
  checkingAppUpdate.value = true;
  try {
    const info = await invoke<AppUpdateInfo | null>("check_app_update");
    appUpdate.value = info;
    if (info?.available) {
      toast("info", `Доступно обновление приложения ${info.latest}.`);
    } else if (!silent) {
      toast("info", "Обновлений приложения нет.");
    }
    return true;
  } catch (error) {
    // A private repository or a missing release is reported as "no update";
    // a silent startup check must not spam the user with network errors.
    if (!silent) {
      toast("error", String(error));
    }
    return false;
  } finally {
    checkingAppUpdate.value = false;
  }
}

export async function applyAppUpdate() {
  if (!appUpdate.value?.available || busy.value) return;
  const info = appUpdate.value;
  const { ok } = await confirmAction({
    title: "Обновить приложение?",
    text: `Приложение обновится до ${info.latest} и перезапустится. Текущая версия — ${info.current}.`,
    confirmLabel: "Обновить",
  });
  if (!ok) return;

  busy.value = true;
  appUpdateProgress.value = "Подготовка обновления…";
  try {
    await invoke("apply_app_update");
    // On success the backend exits this process; the new exe starts itself.
  } catch (error) {
    toast("error", String(error));
    busy.value = false;
    appUpdateProgress.value = null;
  }
}

export async function setTheme(theme: ThemeId) {
  applyTheme(theme);
  try {
    await invoke("set_theme", { theme });
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function setCheckAppUpdates(enabled: boolean) {
  try {
    await invoke("set_check_app_updates", { enabled });
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function openReleasePage(url: string) {
  try {
    await invoke("open_external", { url });
  } catch (error) {
    toast("error", String(error));
  }
}

export async function applyUpdate() {
  if (busy.value || benchRunning.value) return;
  const { ok } = await confirmAction({
    title: "Обновить flowseal-zapret?",
    text: "Обход будет остановлен на время обновления и запущен снова. Пользовательские списки сохранятся.",
    confirmLabel: "Обновить",
  });
  if (!ok) return;

  busy.value = true;
  try {
    await invoke("apply_update");
    updateInfo.value = null;
    await refreshInternal(true);
    toast("success", "flowseal-zapret обновлён.");
  } catch (error) {
    toast("error", String(error));
  } finally {
    busy.value = false;
    updateProgress.value = null;
  }
}

export async function installRuntime() {
  if (busy.value) return;
  busy.value = true;
  try {
    const info = await invoke<UpdateInfo>("check_update");
    updateInfo.value = info;
    if (!info.update_available) {
      toast("warning", "Не удалось найти обновление flowseal-zapret.");
      return;
    }
    await invoke("apply_update");
    await refreshInternal(true);
    toast("success", "flowseal-zapret установлен.");
  } catch (error) {
    toast("error", String(error));
  } finally {
    busy.value = false;
    updateProgress.value = null;
  }
}

export function dismissSetupWizard() {
  wizardDismissed.value = true;
}

async function loadWhatsNew() {
  try {
    whatsNew.value = await invoke<WhatsNew | null>("get_whats_new");
  } catch (error) {
    console.warn("Не удалось загрузить «Что нового»", error);
  }
}

export async function dismissWhatsNew() {
  if (!whatsNew.value) return;
  try {
    await invoke("dismiss_whats_new");
  } catch (error) {
    toast("error", String(error));
  }
  whatsNew.value = null;
}

export async function chooseRuntimeFolder(): Promise<boolean> {
  if (busy.value || benchRunning.value) {
    toast("warning", "Дождитесь завершения текущей операции.");
    return false;
  }

  if (running.value) {
    const { ok } = await confirmAction({
      title: "Изменить папку flowseal-zapret?",
      text: "Для смены папки обход будет остановлен. После изменения запустите его снова вручную.",
      confirmLabel: "Остановить и изменить",
    });
    if (!ok) return false;
    try {
      await invoke("stop");
      await refreshInternal(true);
    } catch (error) {
      toast("error", String(error));
      return false;
    }
  }

  try {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Выберите папку для flowseal-zapret",
    });
    if (typeof selected !== "string") return false;
    const resolved = await invoke<string>("set_runtime_path", { path: selected });
    wizardDismissed.value = false;
    await refreshInternal(true);
    toast("info", `Папка flowseal-zapret: ${resolved}`);
    return true;
  } catch (error) {
    toast("error", String(error));
    return false;
  }
}

export async function useDefaultRuntimeFolder(): Promise<boolean> {
  try {
    const resolved = await invoke<string>("set_runtime_path", { path: "" });
    wizardDismissed.value = false;
    await refreshInternal(true);
    toast("info", `Папка flowseal-zapret: ${resolved}`);
    return true;
  } catch (error) {
    toast("error", String(error));
    return false;
  }
}

export async function toggleAutostart(enabled: boolean) {
  try {
    const result = await invoke<AutostartStatus>("set_autostart", { enabled });
    // The task could not be created and the Run key was used instead: Windows
    // will show a UAC prompt at logon, and the user must know that.
    if (result?.warning) toast("warning", result.warning);
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function toggleCheckUpdates(enabled: boolean) {
  try {
    await invoke("set_check_updates", { enabled });
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function toggleFlashscore(enabled: boolean) {
  try {
    await invoke("set_flashscore_bypass", { enabled });
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function setAutoApply(enabled: boolean) {
  try {
    await invoke("set_bench_auto_apply", { enabled });
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function setGameFilter(mode: string) {
  try {
    await invoke("set_game_filter", { mode });
    await refreshInternal(true);
  } catch (error) {
    toast("error", String(error));
  }
}

export async function openFolder() {
  try {
    await invoke("open_runtime_folder");
  } catch (error) {
    toast("error", String(error));
  }
}

// ---------- backup: export and import of settings ----------

const ARCHIVE_FILTERS = [{ name: "Архив Splitline", extensions: ["zip"] }];

/**
 * Writes the settings archive the user chooses. `includeSecrets` is the explicit
 * opt-in for the proxy key and the subscription URL; without it the backend
 * blanks both fields and leaves the subscription snapshot out.
 */
export async function exportSettings(includeSecrets: boolean) {
  if (backupBusy.value) return;

  let target: string | null;
  try {
    target = await saveDialog({
      title: "Куда сохранить настройки",
      defaultPath: backupFileName(),
      filters: ARCHIVE_FILTERS,
    });
  } catch (error) {
    toast("error", String(error));
    return;
  }
  if (typeof target !== "string") return;

  backupBusy.value = true;
  try {
    const report = await invoke<BackupExportReport>("export_settings", {
      path: target,
      includeSecrets,
    });
    toast("success", `Настройки сохранены: ${fileNameOf(report.path)}`);
  } catch (error) {
    toast("error", String(error));
  } finally {
    backupBusy.value = false;
  }
}

/**
 * Two steps on purpose: the archive is inspected first, the confirmation shows
 * what it contains (and whether it carries secrets), and only then does the
 * backend write anything. The backend copies the current state to `backups/`
 * before it replaces it.
 */
export async function importSettings() {
  if (backupBusy.value) return;

  let picked: string | null;
  try {
    picked = await openDialog({
      title: "Выберите архив с настройками",
      multiple: false,
      directory: false,
      filters: ARCHIVE_FILTERS,
    });
  } catch (error) {
    toast("error", String(error));
    return;
  }
  if (typeof picked !== "string") return;

  backupBusy.value = true;
  try {
    const preview = await invoke<BackupPreview>("inspect_backup", { path: picked });
    const created = preview.created_label ? `Создан: ${preview.created_label}.` : "";
    const { ok } = await confirmAction({
      title: "Импортировать настройки?",
      text: `${preview.summary} ${created} ${preview.warnings.join(" ")} Текущие настройки сохранятся в папке backups.`,
      confirmLabel: "Импортировать",
    });
    if (!ok) return;

    const report = await invoke<BackupImportReport>("import_settings", { path: picked });
    await refreshInternal(true);
    void refreshTgws();
    void refreshXray();
    try {
      benchSummary.value = await invoke<BenchSummary | null>("get_last_bench");
      benchHistory.value = await invoke<BenchSummary[]>("get_bench_history");
    } catch (error) {
      console.warn("Не удалось перечитать историю проверок", error);
    }
    if (report.warnings.length) {
      toast("warning", report.warnings.join(" "));
    } else {
      toast("success", `Настройки импортированы. Копия прежних: ${fileNameOf(report.backup_path)}`);
    }
  } catch (error) {
    toast("error", String(error));
  } finally {
    backupBusy.value = false;
  }
}

// ---------- Telegram proxy ----------

/** Loads everything the Telegram page renders: state, link and log tail. */
export async function refreshTgws() {
  try {
    const [nextStatus, logs] = await Promise.all([
      invoke<TgwsStatus>("get_tgws_status"),
      invoke<string[]>("get_tgws_logs"),
    ]);
    tgwsStatus.value = nextStatus;
    tgwsLogs.value = tgwsLogTail(logs);
    await refreshTgwsLink();
    syncTrayState();
  } catch (error) {
    console.warn("Не удалось обновить состояние Telegram-прокси", error);
  }
}

async function refreshTgwsLink() {
  try {
    tgwsLink.value = await invoke<string>("get_tgws_link");
  } catch (error) {
    console.warn("Не удалось получить ссылку Telegram-прокси", error);
    tgwsLink.value = "";
  }
}

/** Runs the Telegram proxy diagnosis on demand. */
export async function diagnoseTgws() {
  if (tgwsDiagnosing.value) return;
  tgwsDiagnosing.value = true;
  try {
    tgwsDiagnosis.value = await invoke<TgwsDiagnosis>("diagnose_tgws");
  } catch (error) {
    toast("error", String(error));
  } finally {
    tgwsDiagnosing.value = false;
  }
}

export async function checkTgwsUpdate(silent = true, force = false): Promise<boolean> {
  if (tgwsChecking.value) return false;
  tgwsChecking.value = true;
  try {
    const info = await invoke<TgwsUpdateInfo>("check_tgws_update", { force });
    // An explicit check is a request to see updates, so a previously dismissed
    // banner is allowed to come back for this session.
    tgwsUpdate.value = force && info.has_update ? { ...info, dismissed: false } : info;
    if (!silent) {
      toast(
        "info",
        info.has_update && info.latest
          ? `Доступно обновление прокси ${info.latest}.`
          : "Обновлений Telegram-прокси нет.",
      );
    }
    return true;
  } catch (error) {
    if (!silent) toast("error", String(error));
    return false;
  } finally {
    tgwsChecking.value = false;
  }
}

/** One button for all three components: app, flowseal-zapret and the Telegram proxy. */
export async function checkAllUpdates(): Promise<boolean> {
  const results = await Promise.all([
    checkAppUpdate(true),
    checkUpdate(true),
    checkTgwsUpdate(true, true),
  ]);
  if (results.some((ok) => !ok)) {
    toast("warning", "Не удалось проверить все обновления — проверьте сеть.");
    return false;
  }

  const updates: string[] = [];
  if (appUpdate.value?.available) updates.push(`приложение ${appUpdate.value.latest}`);
  if (updateInfo.value?.update_available) {
    updates.push(`flowseal-zapret ${updateInfo.value.latest}`);
  }
  if (tgwsUpdate.value?.has_update && tgwsUpdate.value.latest) {
    updates.push(`Telegram-прокси ${tgwsUpdate.value.latest}`);
  }
  toast(
    updates.length ? "success" : "info",
    updates.length ? `Доступны обновления: ${updates.join(", ")}.` : "Обновлений нет.",
  );
  return true;
}

/** Downloads and installs the newest proxy release (resolved by the backend). */
async function installTgwsBinary(): Promise<void> {
  tgwsProgress.value = "Скачивание прокси…";
  await invoke("install_tgws_update");
  tgwsProgress.value = "Готово";
}

export async function toggleTgws() {
  if (tgwsBusy.value) return;
  const runningNow = tgwsStatus.value?.state === "running";
  tgwsBusy.value = true;
  try {
    if (runningNow) {
      await invoke("stop_tgws");
      toast("info", "Telegram-прокси остановлен.");
    } else {
      if (!tgwsStatus.value?.binary_present) {
        await installTgwsBinary();
      }
      await invoke("start_tgws");
      toast("success", "Telegram-прокси включён.");
    }
  } catch (error) {
    toast("error", String(error));
  } finally {
    await refreshTgws();
    tgwsBusy.value = false;
    tgwsProgress.value = null;
  }
}

export async function restartTgws() {
  if (tgwsBusy.value) return;
  tgwsBusy.value = true;
  try {
    await invoke("restart_tgws");
    toast("success", "Telegram-прокси перезапущен.");
  } catch (error) {
    toast("error", String(error));
  } finally {
    await refreshTgws();
    tgwsBusy.value = false;
  }
}

export async function applyTgwsUpdate() {
  if (tgwsBusy.value) return;
  tgwsBusy.value = true;
  try {
    await installTgwsBinary();
    await refreshTgws();
    await checkTgwsUpdate(true, true);
    toast("success", "Telegram-прокси обновлён.");
  } catch (error) {
    toast("error", String(error));
  } finally {
    tgwsBusy.value = false;
    tgwsProgress.value = null;
  }
}

export async function dismissTgwsUpdate() {
  // Dismiss by the exact mirror tag: a rebuild of the same upstream version
  // must be shown again.
  const tag = tgwsUpdate.value?.latest_tag ?? tgwsUpdate.value?.latest;
  if (!tag) return;
  try {
    await invoke("dismiss_tgws_update", { tag });
    if (tgwsUpdate.value) tgwsUpdate.value = { ...tgwsUpdate.value, dismissed: true };
  } catch (error) {
    toast("error", String(error));
  }
}

export async function regenerateTgwsSecret() {
  if (tgwsBusy.value) return;
  tgwsBusy.value = true;
  try {
    await invoke<string>("regenerate_tgws_secret");
    await refreshTgws();
    toast("success", "Ключ и ссылка изменены.");
  } catch (error) {
    toast("error", String(error));
  } finally {
    tgwsBusy.value = false;
  }
}

async function setTgwsSetting<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try {
    const saved = await invoke<T>(command, args);
    await refreshInternal(true);
    await refreshTgws();
    return saved;
  } catch (error) {
    toast("error", String(error));
    throw error;
  }
}

export function setTgwsHost(host: string) {
  return setTgwsSetting<string>("set_tgws_host", { host });
}

export function setTgwsPort(port: number) {
  return setTgwsSetting<number>("set_tgws_port", { port });
}

export function setTgwsSecret(secret: string) {
  return setTgwsSetting<string>("set_tgws_secret", { secret });
}

/** Moves the proxy to the next free port and restarts it (port busy error). */
export async function switchTgwsPort() {
  if (tgwsBusy.value) return;
  tgwsBusy.value = true;
  try {
    const port = await invoke<number>("use_tgws_fallback_port");
    await refreshTgws();
    toast("success", `Прокси занял порт ${port}. Ссылка в Telegram обновлена.`);
  } catch (error) {
    toast("error", String(error));
  } finally {
    tgwsBusy.value = false;
  }
}

export async function copyTgwsLink() {
  if (!tgwsLink.value) return;
  try {
    await invoke("copy_to_clipboard", { text: tgwsLink.value });
    toast("success", "Ссылка скопирована.");
  } catch (error) {
    toast("error", String(error));
  }
}

export async function openTgwsInTelegram() {
  if (!tgwsLink.value) return;
  try {
    await invoke("open_telegram_link", { url: tgwsLink.value });
  } catch (error) {
    toast("error", String(error));
  }
}

export async function openTgwsLog() {
  try {
    await invoke("open_tgws_log");
  } catch (error) {
    toast("error", String(error));
  }
}

// ---------- Telegram proxy: engine and Cloudflare settings (TG-2) ----------

/**
 * Switches the proxy engine. The backend installs the target binary and
 * restarts a running proxy itself, so this call is the whole action.
 */
export async function setTgwsEngine(engine: string) {
  if (tgwsBusy.value) return;
  tgwsBusy.value = true;
  try {
    const saved = await invoke<string>("set_tgws_engine", { engine });
    await refreshInternal(true);
    await refreshTgws();
    toast(
      "success",
      saved === "rs" ? "Включён новый движок прокси." : "Включён старый движок прокси.",
    );
    return saved;
  } catch (error) {
    toast("error", String(error));
    throw error;
  } finally {
    tgwsBusy.value = false;
  }
}

/** Saves the Cloudflare domain list; the order is the engine's priority. */
export function setTgwsDomains(domains: string[]) {
  return setTgwsSetting<string[]>("set_tgws_domains", { domains });
}

/** Saves the Cloudflare Worker domain; an empty string turns it off. */
export function setTgwsWorkerDomain(domain: string) {
  return setTgwsSetting<string>("set_tgws_worker_domain", { domain });
}

/** Rotates across the domain list instead of always using the first one. */
export function setTgwsBalance(enabled: boolean) {
  return setTgwsSetting<boolean>("set_tgws_balance", { enabled });
}

/** Turns inbound FakeTLS on (a domain) or off (an empty string). */
export function setTgwsFakeTlsDomain(domain: string) {
  return setTgwsSetting<string>("set_tgws_fake_tls_domain", { domain });
}

// ---------- VPN (Xray subscription) ----------

/** Loads what the VPN page renders: state, log tail and the Telegram link. */
export async function refreshXray() {
  try {
    const [nextStatus, logs, link] = await Promise.all([
      invoke<XrayStatus>("get_xray_status"),
      invoke<string[]>("get_xray_logs"),
      invoke<string>("get_xray_telegram_link"),
    ]);
    xrayStatus.value = nextStatus;
    xrayLogs.value = tgwsLogTail(logs);
    xrayLink.value = link;
    syncTrayState();
  } catch (error) {
    console.warn("Не удалось обновить состояние VPN", error);
  }
}

/** Re-downloads the subscription and refreshes the profile list. */
export async function refreshXraySubscription(withPing = true) {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  xrayError.value = null;
  try {
    xrayProfiles.value = await invoke<XrayProfile[]>("refresh_xray_subscription", {
      withPing,
    });
    await refreshInternal(true);
    await refreshXray();
    toast("success", `Список обновлён: ${xrayProfiles.value.length} профилей.`);
  } catch (error) {
    xrayError.value = String(error);
    toast("error", String(error));
  } finally {
    xrayBusy.value = false;
  }
}

/** Loads the cached profile list (with optional pings). */
export async function loadXrayProfiles(withPing = false) {
  xrayError.value = null;
  try {
    xrayProfiles.value = await invoke<XrayProfile[]>("get_xray_profiles", { withPing });
  } catch (error) {
    xrayError.value = String(error);
  }
}

/** Pings every profile and re-sorts nothing: the UI shows the numbers. */
export async function pingXrayProfiles() {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  try {
    xrayProfiles.value = await invoke<XrayProfile[]>("get_xray_profiles", {
      withPing: true,
    });
  } catch (error) {
    toast("error", String(error));
  } finally {
    xrayBusy.value = false;
  }
}

async function setXraySetting<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try {
    const saved = await invoke<T>(command, args);
    await refreshInternal(true);
    await refreshXray();
    return saved;
  } catch (error) {
    toast("error", String(error));
    throw error;
  }
}

export function setXraySubscription(url: string) {
  return setXraySetting<string>("set_xray_subscription", { url });
}

export function selectXrayProfile(index: number) {
  // A manual pick is honoured for five minutes before auto-switch may act.
  manualPickAt = Date.now();
  return setXraySetting<void>("select_xray_profile", { index });
}

export function setXrayPorts(socksPort: number, httpPort: number) {
  return setXraySetting<[number, number]>("set_xray_ports", { socksPort, httpPort });
}

export function toggleXrayAutostart(enabled: boolean) {
  return setXraySetting<void>("set_xray_autostart", { enabled });
}

export async function installXrayCore() {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  try {
    const version = await invoke<string>("install_xray_core");
    await refreshXray();
    toast("success", `Ядро Xray ${version} установлено.`);
  } catch (error) {
    toast("error", String(error));
  } finally {
    xrayBusy.value = false;
  }
}

/** Installs a core archive the user downloaded by hand (blocked networks). */
export async function installXrayCoreFromFile(path: string) {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  try {
    const version = await invoke<string>("install_xray_core_from_file", { path });
    await refreshXray();
    toast("success", `Ядро Xray установлено (${version}).`);
  } catch (error) {
    toast("error", String(error));
  } finally {
    xrayBusy.value = false;
  }
}

export async function toggleXray() {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  try {
    const running = xrayStatus.value?.state === "running";
    await invoke(running ? "stop_xray" : "start_xray");
    await refreshXray();
    if (!running) {
      toast("success", "VPN подключён. Нажми «Подключить Telegram».");
    }
  } catch (error) {
    toast("error", String(error));
    await refreshXray();
  } finally {
    xrayBusy.value = false;
  }
}

export async function restartXray() {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  try {
    await invoke("restart_xray");
    await refreshXray();
  } catch (error) {
    toast("error", String(error));
  } finally {
    xrayBusy.value = false;
  }
}

export async function openXrayInTelegram() {
  try {
    await invoke("open_xray_in_telegram");
  } catch (error) {
    toast("error", String(error));
  }
}

export async function copyXrayLink() {
  if (!xrayLink.value) return;
  try {
    await invoke("copy_to_clipboard", { text: xrayLink.value });
    toast("success", "Ссылка скопирована.");
  } catch (error) {
    toast("error", String(error));
  }
}

/** Replaces the whitelist of sites that must go through the tunnel. */
export async function setXrayProxySites(sites: string[]) {
  return setXraySetting<string[]>("set_xray_proxy_sites", { sites });
}

/** Enables/disables the Windows system proxy (browsers follow the same rules). */
export async function setXraySystemProxy(enabled: boolean) {
  return setXraySetting<void>("set_xray_system_proxy", { enabled });
}

/** Persisted toggle: switch to the fastest live location when the tunnel fails. */
export async function setXrayAutoSwitch(enabled: boolean) {
  return setXraySetting<void>("set_xray_auto_switch", { enabled });
}

/** Rolls the subscription back one fetch, keeping the profiles in sync. */
export async function restorePreviousSubscription() {
  if (xrayBusy.value) return;
  xrayBusy.value = true;
  try {
    const count = await invoke<number>("restore_previous_subscription");
    await loadXrayProfiles(false);
    await refreshXray();
    toast("success", `Возвращена предыдущая подписка: профилей — ${count}.`);
  } catch (error) {
    toast("error", String(error));
  } finally {
    xrayBusy.value = false;
  }
}

/** Merges a domain list from a text file into the whitelist. */
export async function importXraySites(path: string) {
  return setXraySetting<string[]>("import_xray_sites", { path });
}

/** Writes the whitelist to a text file; returns how many domains were saved. */
export async function exportXraySites(path: string) {
  return invoke<number>("export_xray_sites", { path });
}

/** Ready-made service bundles, one `geosite:` category each. */
export async function getXraySiteBundles() {
  return invoke<XraySiteBundle[]>("get_xray_site_bundles");
}

/** Downloads both community domain lists and merges them into one file. */
export async function updateXraySiteBases() {
  if (xrayBusy.value) return null;
  xrayBusy.value = true;
  try {
    const summary = await invoke<XrayBasesSummary>("update_xray_site_bases");
    await refreshXray();
    await refreshInternal(true);
    const restart =
      xrayStatus.value?.state === "running" ? " Перезапустите VPN, чтобы применить." : "";
    toast("success", `Базы обновлены: ${summary.categories} категорий.${restart}`);
    return summary;
  } catch (error) {
    toast("error", String(error));
    return null;
  } finally {
    xrayBusy.value = false;
  }
}

/** Suggests the extra domains a site needs (CDN, static hosts). */
export async function discoverXraySites(site: string) {
  return invoke<XraySiteCandidate[]>("discover_xray_sites", { site });
}
