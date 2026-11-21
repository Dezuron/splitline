import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createApp, nextTick, type App } from "vue";
import { createMemoryHistory, createRouter, type Router } from "vue-router";
import type { AppStatus, TgwsStatus, TgwsUpdateInfo, UpdateInfo, XrayStatus } from "../types";
import OverviewPage from "./OverviewPage.vue";
import {
  appUpdate,
  checkingAppUpdate,
  checkingUpdate,
  status,
  tgwsChecking,
  tgwsStatus,
  tgwsUpdate,
  updateInfo,
  xrayStatus,
} from "../store";
import { toasts } from "../lib/toast";

let app: App | null = null;
let host: HTMLElement | null = null;
let router: Router | null = null;

function makeStatus(overrides: Partial<AppStatus> = {}): AppStatus {
  return {
    app_version: "2.2.0",
    strategies: [],
    generated: [],
    current_strategy: "",
    running: false,
    installed_version: "1.10.4",
    runtime_valid: true,
    ...overrides,
  };
}

function makeXray(overrides: Partial<XrayStatus> = {}): XrayStatus {
  return {
    state: "stopped",
    pid: null,
    last_error: null,
    installed_version: "26.11.9",
    binary_present: true,
    socks_port: 17808,
    http_port: 17809,
    selected: 0,
    profile_name: "",
    client_connections: 0,
    system_proxy: false,
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

function makeTgws(overrides: Partial<TgwsStatus> = {}): TgwsStatus {
  return {
    state: "stopped",
    pid: null,
    last_error: null,
    installed_version: "1.11.4",
    binary_present: true,
    port_conflict: false,
    client_connections_total: 0,
    ...overrides,
  };
}

function makeAppUpdate(available: boolean): void {
  appUpdate.value = {
    current: "2.2.0",
    latest: "2.3.0",
    available,
    url: "",
    notes: "",
    size: 0,
    can_auto_update: false,
  };
}

function makeZapretUpdate(available: boolean): void {
  const info: UpdateInfo = {
    installed: "1.10.4",
    latest: "1.10.5",
    update_available: available,
    changelog: "",
    download_size: 0,
  };
  updateInfo.value = info;
}

function makeTgwsUpdate(hasUpdate: boolean, dismissed = false): void {
  const info: TgwsUpdateInfo = {
    installed: "1.11.4",
    latest: "1.11.5",
    latest_tag: "v1.11.5",
    has_update: hasUpdate,
    asset_name: null,
    asset_url: null,
    asset_size: 0,
    digest: null,
    release_url: null,
    published_at: null,
    dismissed,
  };
  tgwsUpdate.value = info;
}

async function mountPage(): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/", component: { template: "<div />" } },
      { path: "/settings", component: { template: "<div />" } },
    ],
  });
  app = createApp(OverviewPage);
  app.use(router);
  await router.push("/");
  await router.isReady();
  app.mount(host);
  await nextTick();
  return host;
}

interface VersionRow {
  label: string;
  version: string;
  dot: boolean;
}

function versionRows(container: HTMLElement): VersionRow[] {
  return [...container.querySelectorAll(".home-version")].map((row) => ({
    label: row.querySelector(".home-version-label")?.textContent?.trim() ?? "",
    version: row.querySelector(".home-version-value")?.textContent?.trim() ?? "",
    dot: row.querySelector(".home-version-dot") !== null,
  }));
}

describe("OverviewPage hero versions", () => {
  beforeEach(() => {
    status.value = makeStatus();
    xrayStatus.value = makeXray();
    tgwsStatus.value = makeTgws();
    appUpdate.value = null;
    updateInfo.value = null;
    tgwsUpdate.value = null;
    checkingAppUpdate.value = false;
    checkingUpdate.value = false;
    tgwsChecking.value = false;
    toasts.value = [];
    delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  });

  afterEach(() => {
    app?.unmount();
    app = null;
    host?.remove();
    host = null;
    router = null;
  });

  it("shows Splitline, zapret, Xray and Telegram versions", async () => {
    const page = await mountPage();

    expect(versionRows(page)).toEqual([
      { label: "Splitline", version: "2.2.0", dot: false },
      { label: "zapret", version: "1.10.4", dot: false },
      { label: "Xray", version: "26.11.9", dot: false },
      { label: "Telegram", version: "1.11.4", dot: false },
    ]);
  });

  it("reads a missing version as a dash, never blank", async () => {
    status.value = makeStatus({ app_version: "", installed_version: null });
    xrayStatus.value = makeXray({ installed_version: "" });
    tgwsStatus.value = makeTgws({ installed_version: "" });

    const page = await mountPage();

    expect(versionRows(page).map((row) => row.version)).toEqual(["—", "—", "—", "—"]);
  });

  it("marks the components with a known update", async () => {
    makeAppUpdate(true);
    makeZapretUpdate(true);
    makeTgwsUpdate(true);

    const page = await mountPage();

    expect(versionRows(page).map((row) => row.dot)).toEqual([true, true, false, true]);
  });

  it("keeps the Telegram dot off when the release was dismissed", async () => {
    makeTgwsUpdate(true, true);

    const page = await mountPage();

    expect(versionRows(page)[3].dot).toBe(false);
  });

  it("sends the updates button to the settings page", async () => {
    makeAppUpdate(true);
    const page = await mountPage();
    const button = page.querySelector<HTMLButtonElement>(".home-hero button");

    expect(button?.textContent?.trim()).toBe("Доступно 1 обновление");
    button?.click();
    await nextTick();

    await vi.waitFor(() => expect(router?.currentRoute.value.path).toBe("/settings"));
  });

  it("has a quiet refresh button with an accessible name", async () => {
    const page = await mountPage();
    const button = page.querySelector<HTMLButtonElement>(
      '.home-hero button[aria-label="Проверить обновления"]',
    );

    expect(button?.title).toBe("Проверить обновления");
    expect(button?.querySelector("svg")).not.toBeNull();
    expect(button?.disabled).toBe(false);
  });

  it("puts the refresh button at the end of the versions row", async () => {
    const page = await mountPage();
    const row = page.querySelector(".home-hero-bottom");
    const button = row?.querySelector('button[aria-label="Проверить обновления"]');

    expect(row?.firstElementChild?.classList.contains("home-hero-versions")).toBe(true);
    expect(row?.lastElementChild).toBe(button);
    expect(button).not.toBeNull();
  });

  it("spins and disables the refresh button while a check is running", async () => {
    checkingAppUpdate.value = true;
    const page = await mountPage();
    const button = page.querySelector<HTMLButtonElement>(
      '.home-hero button[aria-label="Проверить обновления"]',
    );

    expect(button?.disabled).toBe(true);
    expect(button?.querySelector("svg")?.classList.contains("spinning")).toBe(true);
  });

  it("checks all three components from the hero button", async () => {
    const invoke = vi.fn((cmd: string) => {
      if (cmd === "check_app_update") return Promise.resolve(null);
      if (cmd === "check_update") {
        return Promise.resolve({
          installed: "1.10.4",
          latest: "1.10.4",
          update_available: false,
          changelog: "",
          download_size: 0,
        });
      }
      if (cmd === "check_tgws_update") {
        return Promise.resolve({
          installed: "1.11.4",
          latest: "1.11.4",
          latest_tag: "v1.11.4",
          has_update: false,
          asset_name: null,
          asset_url: null,
          asset_size: 0,
          digest: null,
          release_url: null,
          published_at: null,
          dismissed: false,
        });
      }
      return Promise.resolve(null);
    });
    (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = { invoke };

    const page = await mountPage();
    page
      .querySelector<HTMLButtonElement>('.home-hero button[aria-label="Проверить обновления"]')
      ?.click();

    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(3));
    expect(invoke.mock.calls.map(([cmd]) => cmd).sort()).toEqual([
      "check_app_update",
      "check_tgws_update",
      "check_update",
    ]);
    await vi.waitFor(() =>
      expect(toasts.value[toasts.value.length - 1]?.text).toBe("Обновлений нет."),
    );
  });
});
