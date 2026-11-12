import { beforeEach, describe, expect, it } from "vitest";
import type { AppStatus } from "../types";
import {
  activeRow,
  generatedRows,
  officialRows,
  runtimeReady,
  showSetupWizard,
  showWhatsNew,
  status,
  statusText,
  tgwsStatus,
  whatsNew,
  wizardDismissed,
} from "./state";

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

  it("reports a running bypass", () => {
    status.value = makeStatus({
      runtime_valid: true,
      running: true,
      installed_version: "1.10.4",
    });
    expect(runtimeReady.value).toBe(true);
    expect(showSetupWizard.value).toBe(false);
    expect(statusText.value).toBe("Zapret работает");
  });

  it("reports both services when the bypass and Telegram run", () => {
    status.value = makeStatus({ runtime_valid: true, running: true });
    tgwsStatus.value = {
      state: "running",
      pid: 42,
      last_error: null,
      installed_version: "1.11.2-1",
      binary_present: true,
      port_conflict: false,
      client_connections_total: 1,
    };
    expect(statusText.value).toBe("Zapret и Telegram работают");
  });

  it("reports Telegram alone", () => {
    status.value = makeStatus({ runtime_valid: true, running: false });
    tgwsStatus.value = {
      state: "running",
      pid: 42,
      last_error: null,
      installed_version: "1.11.2-1",
      binary_present: true,
      port_conflict: false,
      client_connections_total: 1,
    };
    expect(statusText.value).toBe("Telegram-прокси работает");
  });

  it("reports everything off", () => {
    status.value = makeStatus({ runtime_valid: true, running: false });
    expect(statusText.value).toBe("Zapret и Telegram выключены");
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
