/**
 * Workaround for tauri-apps/tauri#15799 (unfixed upstream in tauri 2.11):
 * `listen()` resolves when the `plugin:event|listen` invoke returns, but the
 * webview-side listener entry is written by a *separate* eval that lands
 * later. Calling the returned unlisten inside that window throws
 * `TypeError: undefined is not an object (evaluating
 * 'listeners[eventId].handlerId')` — and because the throw happens before
 * the `plugin:event|unlisten` invoke, the backend listener leaks and keeps
 * firing.
 *
 * Deferring the call to a macrotask lets the registration eval land first;
 * the catch + one retry covers entries that never registered at all (e.g.
 * the listen eval was dropped during a navigation).
 */
export function safeUnlisten(unlisten: () => unknown): void {
  const attempt = () =>
    Promise.resolve()
      .then(() => unlisten())
      .catch(() => {});
  window.setTimeout(() => {
    void Promise.resolve()
      .then(() => unlisten())
      .catch(() => window.setTimeout(attempt, 50));
  }, 0);
}
