import type { CandidateResult, HostStatus, Verdict } from "../types";

export const verdictLabel: Record<Verdict, string> = {
  works: "работает",
  partial: "частично",
  broken: "не работает",
  disqualified: "ломает интернет",
};

export const hostStatusLabel: Record<HostStatus, string> = {
  ok: "доступен",
  ssl: "подмена сертификата",
  reset: "сброс соединения",
  timeout: "таймаут",
  dns: "ошибка DNS",
  unsupported: "не поддерживается",
  error: "ошибка",
};

export function formatBytes(bytes: number): string {
  if (!bytes) return "";
  if (bytes >= 1024 * 1024 * 1024) return (bytes / 1024 / 1024 / 1024).toFixed(1) + " ГБ";
  return bytes > 1024 * 1024
    ? (bytes / 1024 / 1024).toFixed(1) + " МБ"
    : (bytes / 1024).toFixed(0) + " КБ";
}

export function formatTimestamp(seconds: number): string {
  if (!seconds) return "";
  return new Intl.DateTimeFormat("ru-RU", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(seconds * 1000));
}

/** What the probe actually saw, in words a user can act on. */
export function hostCause(status: HostStatus): string {
  switch (status) {
    case "ssl":
      return "подмена сертификата (DPI по TLS)";
    case "reset":
      return "сброс соединения на рукопожатии";
    case "timeout":
      return "таймаут — блокировка по адресу или SNI";
    case "dns":
      return "ошибка DNS, стратегия не при чём";
    case "error":
      return "ошибка соединения";
    default:
      return "";
  }
}

/** The most frequent failure among the probed hosts: one short line per row. */
export function strategyCause(result: CandidateResult): string {
  const counts = new Map<string, number>();
  for (const host of result.hosts) {
    if (host.status === "ok" || host.status === "unsupported") continue;
    const text = hostCause(host.status);
    if (!text) continue;
    counts.set(text, (counts.get(text) ?? 0) + 1);
  }
  let best = "";
  let bestCount = 0;
  for (const [text, count] of counts) {
    if (count > bestCount) {
      best = text;
      bestCount = count;
    }
  }
  return best;
}
