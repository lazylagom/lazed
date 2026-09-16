/** UI-local preferences persisted in localStorage. */

const NOTIFY_KEY = "lazed-notify";

/** macOS notification on agent blocked/done transitions (default on). */
export function notificationsEnabled(): boolean {
  return localStorage.getItem(NOTIFY_KEY) !== "off";
}

export function setNotificationsEnabled(on: boolean) {
  localStorage.setItem(NOTIFY_KEY, on ? "on" : "off");
}

/** Settings asks the sidebar to snap back to its default width. */
export const SIDEBAR_RESET_EVENT = "lazed:sidebar-reset";

export function resetSidebarWidth() {
  window.dispatchEvent(new CustomEvent(SIDEBAR_RESET_EVENT));
}
