import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createApp, nextTick, type App } from "vue";
import type { AppStatus } from "../types";
import StrategyPanel from "./StrategyPanel.vue";
import { benchSummary, selected, sortMode, status } from "../store";

let app: App | null = null;
let host: HTMLElement | null = null;

function makeStatus(overrides: Partial<AppStatus> = {}): AppStatus {
  return {
    app_version: "1.5.1",
    strategies: [],
    generated: [],
    current_strategy: "",
    running: true,
    installed_version: "1.10.4",
    runtime_valid: true,
    ...overrides,
  };
}

function strategy(name: string, active = false) {
  return { name, file_name: `${name}.bat`, active, args_preview: name };
}

function mountPanel(): HTMLElement {
  host = document.createElement("div");
  document.body.append(host);
  app = createApp(StrategyPanel);
  app.mount(host);
  return host;
}

function rowNames(container: HTMLElement): string[] {
  return [...container.querySelectorAll(".strategy-row .row-name")].map(
    (element) => element.textContent?.trim() ?? "",
  );
}

describe("StrategyPanel", () => {
  beforeEach(() => {
    status.value = null;
    benchSummary.value = null;
    selected.value = "";
    sortMode.value = "score";
  });

  afterEach(() => {
    app?.unmount();
    app = null;
    host?.remove();
    host = null;
  });

  it("renders the applied strategy as the first row, exactly once", () => {
    status.value = makeStatus({
      strategies: [strategy("alpha"), strategy("beta", true), strategy("gamma")],
      current_strategy: "beta",
    });

    const panel = mountPanel();

    expect(rowNames(panel)).toEqual(["beta", "alpha", "gamma"]);
    expect(panel.querySelector(".strategy-row.active .row-name")?.textContent?.trim()).toBe("beta");
  });

  it("reorders the list when another strategy becomes active", async () => {
    status.value = makeStatus({
      strategies: [strategy("alpha"), strategy("beta", true), strategy("gamma")],
      current_strategy: "beta",
    });
    const panel = mountPanel();
    expect(rowNames(panel)).toEqual(["beta", "alpha", "gamma"]);

    status.value = makeStatus({
      strategies: [strategy("alpha", true), strategy("beta"), strategy("gamma")],
      current_strategy: "alpha",
    });
    await nextTick();

    expect(rowNames(panel)).toEqual(["alpha", "beta", "gamma"]);
  });
});
