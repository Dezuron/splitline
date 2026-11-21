import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toasts } from "./toast";
import {
  TRAY_HINT_KEY,
  TRAY_HINT_TEXT,
  flushTrayToasts,
  pendingTrayToasts,
  queueTrayToast,
  showTrayNotice,
  trayHintOnce,
} from "./tray";

function fakeStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value);
    },
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  toasts.value = [];
  pendingTrayToasts.value = [];
});

afterEach(() => {
  vi.useRealTimers();
});

describe("trayHintOnce", () => {
  it("returns the hint once per installation", () => {
    const storage = fakeStorage();
    expect(trayHintOnce(storage)).toBe(TRAY_HINT_TEXT);
    expect(storage.getItem(TRAY_HINT_KEY)).toBe("1");
    expect(trayHintOnce(storage)).toBeNull();
  });

  it("stays silent when storage was already marked", () => {
    expect(trayHintOnce(fakeStorage({ [TRAY_HINT_KEY]: "1" }))).toBeNull();
  });

  it("does not repeat the hint when storage is broken", () => {
    const broken = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    };
    expect(trayHintOnce(broken)).toBeNull();
    expect(trayHintOnce(null)).toBeNull();
  });
});

describe("tray notices", () => {
  it("shows the notice at once when the window is open", () => {
    showTrayNotice({ message: "VPN: порт занят", visible: true });

    expect(pendingTrayToasts.value).toHaveLength(0);
    expect(toasts.value.map((item) => item.text)).toEqual(["VPN: порт занят"]);
  });

  it("keeps the notice until the window is opened again", () => {
    showTrayNotice({ message: "Обход: нет winws.exe", visible: false });

    expect(toasts.value).toHaveLength(0);

    flushTrayToasts();

    expect(pendingTrayToasts.value).toHaveLength(0);
    expect(toasts.value.map((item) => [item.kind, item.text])).toEqual([
      ["error", "Обход: нет winws.exe"],
    ]);
  });

  it("keeps at most three notices and drops the oldest", () => {
    for (const n of [1, 2, 3, 4, 5]) queueTrayToast("info", `сообщение ${n}`);

    expect(pendingTrayToasts.value.map((item) => item.text)).toEqual([
      "сообщение 3",
      "сообщение 4",
      "сообщение 5",
    ]);
  });

  it("does nothing when there is nothing to flush", () => {
    flushTrayToasts();
    expect(toasts.value).toHaveLength(0);
  });
});
