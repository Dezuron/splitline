import { invoke } from "@tauri-apps/api/core";
import { ref } from "vue";
import { toast, type ToastKind } from "./toast";
import type { TrayNotice } from "../types";

/** Key of the one-time "still in the tray" hint. */
export const TRAY_HINT_KEY = "splitline.tray-hint-shown";
export const TRAY_HINT_TEXT = "Splitline остался в трее. Выйти можно из меню.";

/** Same depth as the toast host: the tray must not bury the screen in toasts. */
const MAX_PENDING = 3;

interface PendingToast {
  kind: ToastKind;
  text: string;
}

/** Notices the tray produced while the window was hidden. */
export const pendingTrayToasts = ref<PendingToast[]>([]);

/** Adds a notice to the next window open instead of showing it to nobody. */
export function queueTrayToast(kind: ToastKind, text: string) {
  pendingTrayToasts.value = [...pendingTrayToasts.value, { kind, text }].slice(-MAX_PENDING);
}

/** Shows a tray notice now when the window is open, otherwise on the next show. */
export function showTrayNotice(notice: TrayNotice) {
  if (notice.visible) {
    toast("error", notice.message);
    return;
  }
  queueTrayToast("error", notice.message);
}

/** Shown when the window becomes visible again. */
export function flushTrayToasts() {
  if (!pendingTrayToasts.value.length) return;
  const items = pendingTrayToasts.value;
  pendingTrayToasts.value = [];
  for (const item of items) toast(item.kind, item.text);
}

type StorageLike = Pick<Storage, "getItem" | "setItem">;

function defaultStorage(): StorageLike | null {
  try {
    return window.localStorage;
  } catch {
    // Storage can be unavailable; the hint is a nicety, not a requirement.
    return null;
  }
}

/**
 * Returns the hint text once per installation and remembers that it was shown.
 * A storage that refuses to work silences the hint instead of repeating it.
 */
export function trayHintOnce(storage: StorageLike | null = defaultStorage()): string | null {
  if (!storage) return null;
  try {
    if (storage.getItem(TRAY_HINT_KEY)) return null;
    storage.setItem(TRAY_HINT_KEY, "1");
    return TRAY_HINT_TEXT;
  } catch {
    return null;
  }
}

export interface TrayRunning {
  tgws: boolean;
  xray: boolean;
}

/**
 * Tells the backend what is running so the tray menu and tooltip stay honest.
 * Zapret is not sent: the backend reads the service manager itself, which also
 * makes a freshly installed runtime or subscription enable its row at once.
 */
export async function syncTray(running: TrayRunning) {
  try {
    await invoke("sync_tray", { running });
  } catch (error) {
    // The tray is an addition, not a requirement: the app keeps working.
    console.warn("Не удалось обновить состояние трея", error);
  }
}
