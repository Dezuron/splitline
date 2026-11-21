import type { TgwsState } from "../types";

/** Human label for the supervisor state shown on the Telegram page. */
export function tgwsStateLabel(state: TgwsState | undefined): string {
  switch (state) {
    case "running":
      return "Прокси включён";
    case "starting":
      return "Запуск…";
    case "stopping":
      return "Остановка…";
    case "error":
      return "Ошибка";
    case "not_installed":
      return "Не установлен";
    default:
      return "Прокси выключен";
  }
}

export type StatusTone = "ok" | "idle" | "warn" | "danger";

export function tgwsTone(state: TgwsState | undefined): StatusTone {
  switch (state) {
    case "running":
      return "ok";
    case "starting":
    case "stopping":
      return "warn";
    case "error":
      return "danger";
    case "not_installed":
      return "warn";
    default:
      return "idle";
  }
}

/**
 * The proxy binds `0.0.0.0` to listen on every interface, but a Telegram link
 * must point at an address the client can dial; loopback is the sane default.
 */
export function displayHost(host: string | undefined): string {
  const value = (host ?? "").trim();
  if (!value || value === "0.0.0.0" || value === "::" || value === "[::]") return "127.0.0.1";
  return value;
}

export function tgwsEndpoint(host: string | undefined, port: number | undefined): string {
  const value = displayHost(host);
  const rendered = value.includes(":") && !value.startsWith("[") ? `[${value}]` : value;
  return `${rendered}:${port ?? 1555}`;
}

/** Version shown for the proxy binary: tag, "вручную" or "не установлен". */
export function tgwsVersionLabel(
  installedVersion: string | undefined | null,
  binaryPresent: boolean | undefined,
): string {
  const version = (installedVersion ?? "").trim();
  if (version) return version;
  return binaryPresent ? "вручную" : "не установлен";
}

/** Keeps the in-memory log buffer bounded, mirroring the backend's ring buffer. */
export function tgwsLogTail(lines: string[], limit = 300): string[] {
  if (lines.length <= limit) return [...lines];
  return lines.slice(lines.length - limit);
}

/** Whether changing `host` exposes the proxy beyond this machine. */
export function isPublicBinding(host: string | undefined): boolean {
  const value = displayHost(host);
  return value !== "127.0.0.1" && value !== "::1" && value !== "localhost";
}

/** The two engines the proxy can run on. */
export type TgwsEngine = "python" | "rs";

/**
 * What the user picks in "Дополнительно". The old build is the safe default;
 * the new one is offered as a beta with its way back.
 */
export const TGWS_ENGINES: { value: TgwsEngine; label: string; note: string }[] = [
  {
    value: "python",
    label: "Старый (проверенный)",
    note: "Работает давно, но занимает больше памяти.",
  },
  {
    value: "rs",
    label: "Новый (beta, меньше памяти)",
    note: "Лёгкий и бесплатный, без подписки. Молодой — при сбоях можно вернуться.",
  },
];

/** Which Telegram hint the page should show, if any. */
export type TelegramProxyPrompt = "vpn" | "needs-proxy" | null;

export interface TelegramPromptInput {
  proxyRunning: boolean;
  /** `tgwsActivity === "active"`: a client is connected right now. */
  proxyActive: boolean;
  /** Connections accepted since the proxy started; 0 means "never". */
  proxyConnectionsTotal: number;
  vpnRunning: boolean;
  vpnConnections: number;
}

/**
 * What to ask the user about Telegram. "Telegram sits on VPN" needs evidence:
 * the proxy saw a client before, that client is gone now, and the VPN is
 * carrying traffic. A proxy that never saw a client gets the neutral
 * "not using the proxy yet" hint instead — unrelated VPN connections must not
 * turn into a claim about Telegram.
 */
export function telegramProxyPrompt(input: TelegramPromptInput): TelegramProxyPrompt {
  if (!input.proxyRunning) return null;
  if (input.proxyConnectionsTotal === 0) return "needs-proxy";
  if (input.proxyActive) return null;
  return input.vpnRunning && input.vpnConnections > 0 ? "vpn" : null;
}

/** Anything unrecognised is the engine the app would actually run. */
export function normalizeTgwsEngine(value: string | undefined | null): TgwsEngine {
  const name = (value ?? "").trim().toLowerCase();
  // `rust` is the backend's alias for the same engine.
  return name === "rs" || name === "rust" ? "rs" : "python";
}

/** Short engine name for the diagnostics card. */
export function tgwsEngineLabel(value: string | undefined | null): string {
  return normalizeTgwsEngine(value) === "rs" ? "Новый (beta)" : "Старый";
}

/** Domains the engine tries in the given order; the backend caps the list. */
export const MAX_CF_DOMAINS = 10;

/** Worker domains for the TCP fallback; the backend caps the list too. */
export const MAX_WORKER_DOMAINS = 10;

/** DC redirects; matches the backend's cap on `--dc-ip` entries. */
export const MAX_DC_IPS = 10;

/** Upstream clamps the buffer to this range (kilobytes). */
export const TGWS_BUF_MIN_KB = 4;
export const TGWS_BUF_MAX_KB = 4096;

/** Ready WebSocket connections per DC; `0` disables the direct DC route. */
export const TGWS_POOL_MIN = 0;
export const TGWS_POOL_MAX = 32;

/** A whole number, or `null` for an empty or non-numeric field. */
function parseWholeNumber(value: string): number | null {
  const text = value.trim();
  return /^\d+$/.test(text) ? Number(text) : null;
}

/**
 * Why the buffer/pool pair cannot be saved, in one sentence; empty when both
 * are valid. The rules mirror the backend, so the page can explain a typo
 * before the proxy is restarted.
 */
export function tgwsLimitsError(bufKb: string, poolSize: string): string {
  const buf = parseWholeNumber(bufKb);
  if (buf === null || buf < TGWS_BUF_MIN_KB || buf > TGWS_BUF_MAX_KB) {
    return `Буфер — целое число от ${TGWS_BUF_MIN_KB} до ${TGWS_BUF_MAX_KB} КБ.`;
  }
  const pool = parseWholeNumber(poolSize);
  if (pool === null || pool < TGWS_POOL_MIN || pool > TGWS_POOL_MAX) {
    return `Пул — целое число от ${TGWS_POOL_MIN} до ${TGWS_POOL_MAX}.`;
  }
  return "";
}
