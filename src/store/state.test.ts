import { beforeEach, describe, expect, it } from "vitest";
import type { AppStatus, TgwsStatus, XrayStatus } from "../types";
import {
  activeRow,
  activeServiceCount,
  benchRunning,
  formatAgo,
  generatedRows,
  isRecentlyActive,
  officialRows,
  plural,
  runtimeReady,
  secondsAgo,
  servicesStatus,
  showSetupWizard,
  showWhatsNew,
  status,
  statusText,
  statusTone,
  tgwsActiveConnections,
  tgwsActivity,
  tgwsLastActivityLabel,
  tgwsStatus,
  tgwsStatusText,
  vpnActivity,
  vpnStatusText,
  whatsNew,
  wizardDismissed,
  xrayLastTrafficAt,
  xrayStatus,
  zapretStatusText,
  zapretTone,
} from "./state";

const NOW_SEC = () => Math.floor(Date.now() / 1000);

function makeStatus(overrides: Partial<AppStatus> = {}): AppStatus {
  return {
    app_version: "1.2.0",
    strategies: [],
    generated: [],
    current_strategy: "",
    running: false,
    installed_version: null,
    runtime_valid: false,
    ...overrides,
  };
}

function makeTgws(overrides: Partial<TgwsStatus> = {}): TgwsStatus {
  return {
    state: "running",
    pid: 42,
    last_error: null,
    installed_version: "1.11.2-1",
    binary_present: true,
    port_conflict: false,
    client_connections_total: 0,
    ...overrides,
  };
}

function makeXray(overrides: Partial<XrayStatus> = {}): XrayStatus {
  return {
    state: "running",
    pid: 7,
    last_error: null,
    installed_version: "25.1.1",
    binary_present: true,
    socks_port: 17808,
    http_port: 17809,
    selected: 0,
    profile_name: "Германия",
    client_connections: 0,
    system_proxy: true,
    fetched_at: 0,
    subscription_upload: 0,
    subscription_download: 0,
    subscription_total: 0,
    subscription_expire: 0,
    subscription_page: "",
    has_previous: false,
    ...overrides,
  };
}

/** The 8 combinations from UI-2: chip text and tone for each. */
const MATRIX: Array<{
  name: string;
  zapret: boolean;
  telegram: "off" | "idle" | "active" | "unknown";
  vpn: "off" | "idle" | "active";
  text: string;
  tone: "ok" | "idle" | "danger";
}> = [
  {
    name: "everything off",
    zapret: false,
    telegram: "off",
    vpn: "off",
    text: "Все сервисы выключены",
    tone: "idle",
  },
  {
    name: "only zapret",
    zapret: true,
    telegram: "off",
    vpn: "off",
    text: "Zapret работает",
    tone: "ok",
  },
  {
    name: "proxy without clients",
    zapret: false,
    telegram: "idle",
    vpn: "off",
    text: "Прокси включён · Telegram не подключён",
    tone: "idle",
  },
  {
    name: "proxy with a client",
    zapret: false,
    telegram: "active",
    vpn: "off",
    text: "Telegram работает",
    tone: "ok",
  },
  {
    name: "vpn without traffic",
    zapret: false,
    telegram: "off",
    vpn: "idle",
    text: "VPN включён · трафика нет",
    tone: "idle",
  },
  {
    name: "vpn with traffic",
    zapret: false,
    telegram: "off",
    vpn: "active",
    text: "VPN работает",
    tone: "ok",
  },
  {
    name: "proxy and vpn, no activity",
    zapret: false,
    telegram: "idle",
    vpn: "idle",
    text: "Telegram и VPN включены",
    tone: "idle",
  },
  {
    name: "everything on",
    zapret: true,
    telegram: "active",
    vpn: "active",
    text: "Zapret, Telegram и VPN работают",
    tone: "ok",
  },
];

function strategy(name: string, active = false) {
  return { name, file_name: `${name}.bat`, active, args_preview: name };
}

function generated(name: string, parent: string, active = false) {
  return { name, parent, score: 50, active, args_preview: name };
}

describe("store state", () => {
  beforeEach(() => {
    status.value = null;
    tgwsStatus.value = null;
    xrayStatus.value = null;
    xrayLastTrafficAt.value = 0;
    wizardDismissed.value = false;
    whatsNew.value = null;
  });

  it("shows the setup wizard until zapret is installed or dismissed", () => {
    status.value = makeStatus();
    expect(runtimeReady.value).toBe(false);
    expect(showSetupWizard.value).toBe(true);
    expect(statusText.value).toBe("flowseal-zapret не установлен");

    wizardDismissed.value = true;
    expect(showSetupWizard.value).toBe(false);
  });

  describe("status matrix", () => {
    for (const row of MATRIX) {
      it(`reads "${row.text}" for ${row.name}`, () => {
        status.value = makeStatus({ runtime_valid: true, running: row.zapret });
        if (row.telegram !== "off") {
          tgwsStatus.value = makeTgws({
            active_connections:
              row.telegram === "active" ? 1 : row.telegram === "idle" ? 0 : undefined,
            last_client_at: row.telegram === "active" ? NOW_SEC() - 12 : null,
          });
        }
        if (row.vpn !== "off") {
          xrayStatus.value = makeXray();
          if (row.vpn === "active") xrayLastTrafficAt.value = NOW_SEC();
        }

        expect(statusText.value).toBe(row.text);
        expect(statusTone.value).toBe(row.tone);
      });
    }
  });

  it("never calls a cumulative counter an active connection", () => {
    status.value = makeStatus({ runtime_valid: true });
    // A backend without `active=`/`last_client_at` only has the cumulative total.
    tgwsStatus.value = makeTgws({ client_connections_total: 7 });

    expect(tgwsActivity.value).toBe("unknown");
    expect(tgwsActiveConnections.value).toBeNull();
    expect(tgwsStatusText.value).toBe("Прокси включён");
    expect(statusText.value).toBe("Прокси включён");
    expect(statusTone.value).toBe("idle");
  });

  it("estimates activity from the last client event when active= is missing", () => {
    status.value = makeStatus({ runtime_valid: true });
    tgwsStatus.value = makeTgws({ last_client_at: NOW_SEC() - 12 });

    expect(tgwsActivity.value).toBe("active");
    expect(tgwsActiveConnections.value).toBe(1);
    expect(tgwsStatusText.value).toBe("Telegram работает через прокси · 1 соединение");
  });

  it("drops Telegram back to idle after five quiet minutes", () => {
    status.value = makeStatus({ runtime_valid: true });
    tgwsStatus.value = makeTgws({ last_client_at: NOW_SEC() - 6 * 60 });

    expect(tgwsActivity.value).toBe("idle");
    expect(statusText.value).toBe("Прокси включён · Telegram не подключён");
    expect(tgwsLastActivityLabel.value).toBe("6 мин назад");
  });

  it("keeps a running bypass green while Telegram is only on", () => {
    status.value = makeStatus({ runtime_valid: true, running: true });
    tgwsStatus.value = makeTgws({ active_connections: 0 });

    expect(statusText.value).toBe("Zapret работает · Telegram не подключён");
    expect(statusTone.value).toBe("ok");
  });

  it("describes the VPN page with the same rules", () => {
    xrayStatus.value = makeXray();
    expect(vpnStatusText.value).toBe("VPN включён · трафика нет");
    expect(vpnActivity.value).toBe("idle");

    xrayLastTrafficAt.value = NOW_SEC();
    expect(vpnStatusText.value).toBe("VPN работает");
    expect(vpnActivity.value).toBe("active");

    xrayStatus.value = makeXray({ state: "error" });
    expect(vpnStatusText.value).toBe("Ошибка");
  });

  it("describes the Zapret page by the bypass alone", () => {
    status.value = makeStatus();
    expect(runtimeReady.value).toBe(false);
    expect(zapretStatusText.value).toBe("flowseal-zapret не установлен");
    expect(zapretTone.value).toBe("danger");

    status.value = makeStatus({ runtime_valid: true });
    expect(zapretStatusText.value).toBe("Обход выключен");
    expect(zapretTone.value).toBe("idle");

    status.value = makeStatus({ runtime_valid: true, running: true });
    expect(zapretStatusText.value).toBe("Обход включён");
    expect(zapretTone.value).toBe("ok");

    benchRunning.value = true;
    expect(zapretStatusText.value).toBe("Идёт проверка стратегий");
    expect(zapretTone.value).toBe("idle");
    benchRunning.value = false;

    status.value = null;
    expect(zapretStatusText.value).toBe("Загрузка…");
    expect(zapretTone.value).toBe("idle");
  });

  it("counts only confirmed services as active", () => {
    status.value = makeStatus({ runtime_valid: true });
    tgwsStatus.value = makeTgws({ active_connections: 2, last_client_at: NOW_SEC() });
    xrayStatus.value = makeXray();

    expect(activeServiceCount.value).toBe(1);
    expect(servicesStatus.value.map((service) => service.state)).toEqual([
      "idle",
      "active",
      "idle",
    ]);
  });

  it("formats the last activity in plain words", () => {
    const now = 1_700_000_000_000;
    const nowSec = Math.floor(now / 1000);

    expect(secondsAgo(nowSec - 12, now)).toBe(12);
    expect(secondsAgo(null, now)).toBeNull();
    expect(isRecentlyActive(nowSec - 12, now)).toBe(true);
    expect(isRecentlyActive(nowSec - 6 * 60, now)).toBe(false);
    // Millisecond timestamps are tolerated.
    expect(isRecentlyActive(now - 12_000, now)).toBe(true);
    expect(formatAgo(nowSec - 12, now)).toBe("12 с назад");
    expect(formatAgo(nowSec - 3, now)).toBe("только что");
    expect(formatAgo(nowSec - 5 * 60, now)).toBe("5 мин назад");
    expect(formatAgo(null, now)).toBe("—");
    expect(plural(1, "соединение", "соединения", "соединений")).toBe("соединение");
    expect(plural(2, "соединение", "соединения", "соединений")).toBe("соединения");
    expect(plural(11, "соединение", "соединения", "соединений")).toBe("соединений");
  });

  it("shows What's new only after the setup wizard", () => {
    status.value = makeStatus();
    whatsNew.value = { version: "1.3.0", notes: "- test", url: "https://example" };
    expect(showWhatsNew.value).toBe(false);

    status.value = makeStatus({ runtime_valid: true, installed_version: "1.10.4" });
    expect(showWhatsNew.value).toBe(true);
  });

  it("sorts official strategies by score", () => {
    status.value = makeStatus({
      strategies: [
        { name: "alpha", file_name: "alpha.bat", active: false, args_preview: "a" },
        { name: "beta", file_name: "beta.bat", active: false, args_preview: "b" },
      ],
    });
    expect(officialRows.value.map((row) => row.name)).toEqual(["alpha", "beta"]);
  });

  it("takes the active strategy out of the sorted official list", () => {
    status.value = makeStatus({
      strategies: [strategy("alpha"), strategy("beta", true), strategy("gamma")],
      current_strategy: "beta",
    });
    expect(officialRows.value.map((row) => row.name)).toEqual(["alpha", "gamma"]);
  });

  it("keeps the plain order when no strategy is active", () => {
    status.value = makeStatus({ strategies: [strategy("beta"), strategy("alpha")] });
    expect(officialRows.value.map((row) => row.name)).toEqual(["alpha", "beta"]);
  });

  it("takes an active experimental strategy out of its group", () => {
    status.value = makeStatus({
      generated: [generated("exp-1", "alpha", true), generated("exp-2", "alpha")],
      current_strategy: "exp-1",
    });
    expect(generatedRows.value.map((row) => row.name)).toEqual(["exp-2"]);
  });

  it("exposes the active strategy as a pinned row", () => {
    status.value = makeStatus({
      strategies: [strategy("alpha"), strategy("beta", true)],
      current_strategy: "beta",
    });
    expect(activeRow.value?.name).toBe("beta");
    expect(activeRow.value?.generated).toBe(false);
  });

  it("pins an active experimental strategy with its parent", () => {
    status.value = makeStatus({
      generated: [generated("exp-1", "alpha", true)],
      current_strategy: "exp-1",
    });
    expect(activeRow.value).toMatchObject({
      name: "exp-1",
      generated: true,
      parent: "alpha",
    });
  });

  it("has no pinned row while nothing is applied", () => {
    status.value = makeStatus({ strategies: [strategy("alpha")] });
    expect(activeRow.value).toBeNull();
  });
});
