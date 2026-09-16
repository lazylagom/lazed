import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Terminal } from "@xterm/xterm";
import { translateKeyEvent } from "./keymap";

/**
 * IME Overlay — bypasses xterm.js keyboard handling entirely so that
 * Korean (Hangul) IME composition works inside the Tauri webview.
 *
 * Background:
 *   In the Tauri WebView (WKWebView) xterm.js' internal textarea does not
 *   receive compositionstart/update/end events reliably, so "안녕하세요"
 *   arrives scrambled ("ㅇㄴㅎ요").
 *
 * Approach (ported from laze, see laze docs/korean-ime.md):
 *   A dedicated invisible <input> takes keyboard focus; all key input and
 *   IME composition is handled there and forwarded straight to the PTY.
 *   xterm.js is used for rendering and mouse events only.
 */
export class IMEOverlay {
  private input: HTMLInputElement;
  private preview: HTMLDivElement;
  private isComposing = false;
  private isDisposed = false;
  private disposables: (() => void)[] = [];
  private pendingHangulJamo = "";
  private pendingHangulFlushTimerId: number | null = null;
  private pendingHangulConsumedByComposition = false;
  private suppressedPostCompositionInput: string | null = null;
  private keydownAwaitingInput: {
    code: string;
    key: string;
    shift: boolean;
  } | null = null;
  private swallowedKey: {
    code: string;
    key: string;
    shift: boolean;
    at: number;
  } | null = null;
  private lastQueuedJamoValue = "";
  private lastQueuedJamoAt = 0;

  constructor(
    private terminal: Terminal,
    private container: HTMLElement,
    private writeToPty: (data: string) => void,
  ) {
    // block xterm from processing any keyboard events itself
    terminal.attachCustomKeyEventHandler(() => false);

    this.input = this.createInput();
    this.preview = this.createPreview();

    container.style.position = "relative";
    container.appendChild(this.input);
    container.appendChild(this.preview);

    this.setupCompositionEvents();
    this.setupKeyboardEvents();
    this.setupClipboardEvents();
    this.setupFileDropEvents();
    this.setupFocusManagement();
  }

  /* ── DOM elements ─────────────────────────── */

  private createInput(): HTMLInputElement {
    const el = document.createElement("input");
    el.className = "ime-overlay-input";
    el.setAttribute("autocapitalize", "off");
    el.setAttribute("autocomplete", "off");
    el.setAttribute("autocorrect", "off");
    el.setAttribute("spellcheck", "false");
    return el;
  }

  private createPreview(): HTMLDivElement {
    const el = document.createElement("div");
    el.className = "ime-composition-preview";
    return el;
  }

  /* ── Composition — core of Hangul input ───── */

  private setupCompositionEvents(): void {
    this.on(this.input, "compositionstart", () => {
      this.debugLog("compositionstart");
      this.clearPendingHangulFlushTimer();
      if (this.pendingHangulJamo) {
        this.pendingHangulConsumedByComposition = true;
      }
      this.suppressedPostCompositionInput = null;
      this.isComposing = true;
      this.preview.style.display = "block";
    });

    this.on(this.input, "compositionupdate", (e: CompositionEvent) => {
      this.preview.textContent = e.data ?? "";
    });

    this.on(this.input, "compositionend", (e: CompositionEvent) => {
      this.debugLog("compositionend", e.data);
      const committed = e.data ?? "";
      this.isComposing = false;
      this.preview.style.display = "none";
      this.preview.textContent = "";

      if (committed && containsHangul(committed)) {
        // Right after an input-source switch, WKWebView splits a syllable
        // across sessions: the first jamo commits as plain insertText and
        // the rest arrive as separate one-jamo compositions. Route every
        // Hangul commit back through the jamo buffer so the fragments
        // recompose — the jamo stream keeps its order regardless of where
        // the composition boundaries fell.
        //
        // Steady state is different: a committed string of complete
        // syllables is IME-final, so with nothing pending it can reach the
        // PTY immediately instead of trailing one syllable behind.
        if (!this.pendingHangulJamo && isCompleteSyllableText(committed)) {
          this.writeToPty(committed);
        } else {
          this.queuePendingHangulReplacement(jamoText(committed));
          this.flushCompletedPendingUnits();
        }
        this.suppressedPostCompositionInput = committed;
      } else {
        const { text: pending, consumedByComposition } =
          this.takePendingHangulJamo();
        if (!consumedByComposition && pending && pending !== committed) {
          this.writeToPty(pending);
        }
        if (committed) {
          this.writeToPty(committed);
          this.suppressedPostCompositionInput = committed;
        }
      }
      this.input.value = "";
    });
  }

  /* ── Keyboard — non-IME key handling ──────── */

  private setupKeyboardEvents(): void {
    this.on(this.input, "keydown", (e: KeyboardEvent) => {
      this.debugLog("keydown", {
        key: e.key,
        code: e.code,
        keyCode: e.keyCode,
        isComposing: e.isComposing,
      });
      // A letter keydown that produced no input event before the next
      // keydown was eaten — WKWebView consumes the first keystroke while
      // rebuilding the input context after an input-source switch. Keep its
      // physical key code so the jamo can be recovered when Hangul input
      // arrives right after.
      if (this.keydownAwaitingInput) {
        this.swallowedKey = {
          ...this.keydownAwaitingInput,
          at: Date.now(),
        };
        this.keydownAwaitingInput = null;
      }

      // never interfere while the IME is composing
      if (this.isComposing || e.isComposing || e.keyCode === 229) return;

      if (
        !e.ctrlKey &&
        !e.altKey &&
        !e.metaKey &&
        e.key.length === 1 &&
        (/^[a-zA-Z]$/.test(e.key) || isHangulJamoCharacter(e.key))
      ) {
        this.keydownAwaitingInput = {
          code: e.code,
          key: e.key,
          shift: e.shiftKey,
        };
      }

      // in some environments the first keydown after switching to Hangul
      // arrives before compositionstart; forwarding the jamo as a plain
      // character would split "체" into "ㅊㅔ" — defer Hangul keys to the IME
      if (this.shouldDeferKeydownToIme(e)) return;

      // plain text input is forwarded from the `input` event only, once the
      // string is final — otherwise "한글 right after latin" splits jamo
      const isPlainTextInput =
        !e.ctrlKey && !e.altKey && !e.metaKey && e.key.length === 1;
      if (isPlainTextInput) return;

      this.flushPendingHangulJamo();

      // Cmd combos -> keep browser defaults (copy/paste/…)
      if (e.metaKey) {
        this.handleMetaKey(e);
        return;
      }

      const data = translateKeyEvent(e, this.isApplicationCursorMode());
      if (data !== null) {
        e.preventDefault();
        this.writeToPty(data);
        this.input.value = "";
      }
    });

    // plain character input that wasn't handled in keydown
    this.on(this.input, "input", (e: Event) => {
      const inputEvent = e as InputEvent;
      const inputType = inputEvent.inputType ?? "";
      this.debugLog("input", {
        inputType,
        data: inputEvent.data,
        isComposing: inputEvent.isComposing,
        value: this.input.value,
      });
      this.keydownAwaitingInput = null;

      // WKWebView can run an entire composition without any composition*
      // events — this is the path the first syllable takes right after the
      // input source is switched to Hangul. The in-progress text arrives as
      // insertReplacementText / insertCompositionText / isComposing input;
      // merge it into the pending buffer and flag it as session-owned so a
      // real compositionend (if one ever arrives) wins over the buffer.
      const isMarkedInput =
        inputEvent.isComposing ||
        inputType === "insertCompositionText" ||
        inputType === "insertReplacementText" ||
        inputType === "insertFromComposition";

      if (!this.isComposing && isMarkedInput) {
        const value = this.input.value || inputEvent.data || "";
        this.input.value = "";
        if (!value) return;
        this.recoverSwallowedKey(value, true);
        if (containsOnlyHangulJamo(value)) {
          this.queuePendingHangulJamo(value);
        } else if (containsHangul(value)) {
          this.queuePendingHangulReplacement(value);
        } else {
          this.flushPendingHangulJamo();
          this.writeToPty(value);
        }
        // completed units echo while the user keeps typing — only the
        // trailing unit can still merge, so it stays buffered for the
        // idle flush
        this.flushCompletedPendingUnits();
        this.pendingHangulConsumedByComposition = true;
        return;
      }

      const isCompositionInput =
        inputEvent.isComposing ||
        inputType === "insertCompositionText" ||
        inputType === "insertFromComposition";

      if (this.isComposing || isCompositionInput || !this.input.value) return;

      if (this.shouldSuppressPostCompositionInput(this.input.value)) {
        this.input.value = "";
        return;
      }

      const value = this.input.value;
      this.recoverSwallowedKey(value, false);

      if (containsOnlyHangulJamo(value)) {
        this.queuePendingHangulJamo(value);
        this.flushCompletedPendingUnits();
        this.input.value = "";
        return;
      }

      this.flushPendingHangulJamo();
      this.writeToPty(value);
      this.input.value = "";
    });
  }

  /**
   * Cmd key combos. Most keep their browser default behavior, but Cmd+C
   * copies the xterm selection when one exists.
   */
  private handleMetaKey(e: KeyboardEvent): void {
    if (e.key === "c") {
      const selection = this.terminal.getSelection();
      if (selection) {
        e.preventDefault();
        navigator.clipboard.writeText(selection).catch(() => {});
      }
    }
    // Cmd+V, Cmd+A, ... use the browser default behavior
  }

  /* ── Clipboard ────────────────────────────── */

  private setupClipboardEvents(): void {
    this.on(this.input, "paste", (e: ClipboardEvent) => {
      e.preventDefault();
      const text = e.clipboardData?.getData("text");
      this.flushPendingHangulJamo();
      if (text) this.writeToPty(text);
      this.input.value = "";
    });
  }

  private setupFileDropEvents(): void {
    this.on(this.container, "dragover", (e: DragEvent) => {
      if (!hasDroppedFiles(e.dataTransfer)) {
        return;
      }
      e.preventDefault();
    });

    this.on(this.container, "drop", (e: DragEvent) => {
      const paths = extractDroppedPaths(e.dataTransfer);
      if (!paths.length) {
        return;
      }
      e.preventDefault();
      this.writeDroppedPaths(paths);
    });

    void getCurrentWindow()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") {
          return;
        }
        if (
          !this.containsPhysicalPosition(
            event.payload.position.x,
            event.payload.position.y,
          )
        ) {
          return;
        }
        this.writeDroppedPaths(event.payload.paths);
      })
      .then((unlisten) => {
        if (this.isDisposed) {
          unlisten();
          return;
        }
        this.disposables.push(unlisten);
      })
      .catch(() => {
        // no file-drop bridge in browser tests / non-Tauri environments
      });
  }

  private queuePendingHangulJamo(value: string): void {
    if (!value) return;

    // some input methods resend the last jamo as its own event — only skip
    // it when it arrives back-to-back with the previous queue, so a real
    // repeated keystroke (ㄴㄴ in 안녕) is never dropped
    const now = Date.now();
    const isResend =
      value === this.lastQueuedJamoValue &&
      now - this.lastQueuedJamoAt < JAMO_RESEND_WINDOW_MS;
    this.lastQueuedJamoValue = value;
    this.lastQueuedJamoAt = now;

    if (!this.pendingHangulJamo) {
      this.pendingHangulJamo = value;
      this.pendingHangulConsumedByComposition = false;
    } else if (value.length > 1 && value.startsWith(this.pendingHangulJamo)) {
      // some input methods resend the accumulated string
      this.pendingHangulJamo = value;
    } else if (!(isResend && this.pendingHangulJamo.endsWith(value))) {
      this.pendingHangulJamo += value;
    }

    this.showPendingPreview();
    this.schedulePendingHangulFlush();
  }

  /**
   * insertReplacementText carries the IME's in-progress text as
   * already-composed text. When it extends the pending buffer's trailing
   * unit ("ㅎ" → "하" → "한") the unit is replaced; when it starts a fresh
   * unit ("한" → "그") it is appended.
   */
  private queuePendingHangulReplacement(value: string): void {
    const chars = [...this.pendingHangulJamo];
    const unitStart = lastComposingUnitStart(chars);
    const unit = chars.slice(unitStart).join("");
    if (unit && jamoText(value).startsWith(jamoText(unit))) {
      this.pendingHangulJamo = chars.slice(0, unitStart).join("") + value;
    } else {
      this.pendingHangulJamo += value;
    }
    this.pendingHangulConsumedByComposition = false;
    this.showPendingPreview();
    this.schedulePendingHangulFlush();
  }

  /**
   * A swallowed letter keydown is recovered once Hangul input arrives right
   * after it: the pressed jamo (or the physical key code mapped through the
   * 2-set layout) is prepended ahead of the pending buffer so the syllable
   * composes correctly.
   */
  private recoverSwallowedKey(value: string, isMarkedInput: boolean): void {
    const key = this.swallowedKey;
    this.swallowedKey = null;
    if (!key || !containsHangul(value)) return;
    if (Date.now() - key.at > SWALLOWED_KEY_RECOVERY_MS) return;

    const jamo = isHangulJamoCharacter(key.key)
      ? key.key
      : (key.shift ? KOREAN_2SET_SHIFT_KEYCODE : KOREAN_2SET_KEYCODE)[key.code];
    if (!jamo) return;

    // composition text that already begins with the lost jamo means the IME
    // did deliver it — prepending again would double the initial consonant
    if (isMarkedInput && jamoText(value).startsWith(jamo)) return;

    this.pendingHangulJamo = jamo + this.pendingHangulJamo;
    this.pendingHangulConsumedByComposition = false;
    this.showPendingPreview();
    this.schedulePendingHangulFlush();
  }

  /**
   * The WKWebView jamo/replacement paths never fire compositionupdate, so
   * the in-progress syllable would otherwise be invisible until the PTY
   * echoes the flushed text — mirror it in the composition preview.
   */
  private showPendingPreview(): void {
    const chars = [...this.pendingHangulJamo];
    const unit = chars.slice(lastComposingUnitStart(chars)).join("");
    this.preview.textContent = composeHangulJamo(unit);
    this.preview.style.display = unit ? "block" : "none";
  }

  /**
   * Composition commits keep flowing through the pending buffer, but only
   * the trailing unit can still merge with a following fragment — every
   * unit before it is final and can reach the PTY immediately, so earlier
   * syllables echo while the user keeps typing instead of waiting for the
   * idle flush.
   */
  private flushCompletedPendingUnits(): void {
    if (!this.pendingHangulJamo) return;
    const chars = [...this.pendingHangulJamo];
    const unitStart = lastComposingUnitStart(chars);
    if (unitStart <= 0) return;

    const completed = composeHangulJamo(chars.slice(0, unitStart).join(""));
    this.pendingHangulJamo = chars.slice(unitStart).join("");
    if (completed) this.writeToPty(completed);
    this.showPendingPreview();
  }

  private schedulePendingHangulFlush(): void {
    this.clearPendingHangulFlushTimer();
    this.pendingHangulFlushTimerId = window.setTimeout(() => {
      this.pendingHangulFlushTimerId = null;
      if (this.isComposing) return;
      this.flushPendingHangulJamo();
    }, HANGUL_JAMO_FLUSH_DELAY_MS);
  }

  private clearPendingHangulFlushTimer(): void {
    if (this.pendingHangulFlushTimerId !== null) {
      clearTimeout(this.pendingHangulFlushTimerId);
      this.pendingHangulFlushTimerId = null;
    }
  }

  private takePendingHangulJamo(): {
    text: string;
    consumedByComposition: boolean;
  } {
    this.clearPendingHangulFlushTimer();
    if (!this.pendingHangulJamo) {
      return { text: "", consumedByComposition: false };
    }

    const text = composeHangulJamo(this.pendingHangulJamo);
    const consumedByComposition = this.pendingHangulConsumedByComposition;
    this.pendingHangulJamo = "";
    this.pendingHangulConsumedByComposition = false;
    this.hidePendingPreview();
    return { text, consumedByComposition };
  }

  private discardPendingHangulJamo(): void {
    this.clearPendingHangulFlushTimer();
    this.pendingHangulJamo = "";
    this.pendingHangulConsumedByComposition = false;
    this.hidePendingPreview();
  }

  private hidePendingPreview(): void {
    if (this.isComposing) return;
    this.preview.style.display = "none";
    this.preview.textContent = "";
  }

  private flushPendingHangulJamo(): void {
    const { text } = this.takePendingHangulJamo();
    if (!text) return;
    this.writeToPty(text);
  }

  private shouldSuppressPostCompositionInput(value: string): boolean {
    if (!this.suppressedPostCompositionInput) {
      return false;
    }

    const shouldSuppress = value === this.suppressedPostCompositionInput;
    this.suppressedPostCompositionInput = null;
    return shouldSuppress;
  }

  /** whether the xterm terminal is in Application Cursor Keys Mode (DECCKM) */
  private isApplicationCursorMode(): boolean {
    try {
      // biome-ignore lint/suspicious/noExplicitAny: xterm.js internal modes API
      return (this.terminal as any).modes?.applicationCursorKeysMode ?? false;
    } catch (error) {
      console.warn("failed to check Application Cursor Keys Mode:", error);
      return false;
    }
  }

  private shouldDeferKeydownToIme(e: KeyboardEvent): boolean {
    if (e.metaKey || e.ctrlKey) return false;

    if (e.key === "Dead") {
      return true;
    }

    // on macOS some characters (e.g. ~) go through an Option dead-key path
    // where keydown looks like Escape and the real char arrives via `input`;
    // forwarding Escape first leaks sequences like `;4;27~` into vim/tmux
    if (e.altKey && e.code === "Escape" && e.key === "Escape") {
      return true;
    }

    if (e.altKey) return false;
    if (e.key.length !== 1) return false;
    return isHangulCharacter(e.key);
  }

  /* ── Focus management ─────────────────────── */

  /**
   * Keep the xterm cursor visually active while the overlay input holds
   * focus: xterm only adds the 'focus' class when its own textarea is
   * focused, so we maintain it manually.
   */
  private setupFocusManagement(): void {
    const xtermEl = this.container.querySelector(
      ".xterm",
    ) as HTMLElement | null;

    if (xtermEl) {
      this.on(this.input, "focus", () => xtermEl.classList.add("focus"));
      this.on(this.input, "blur", () => {
        xtermEl.classList.remove("focus");
        this.resetImeStateOnBlur();
      });
    } else {
      this.on(this.input, "blur", () => this.resetImeStateOnBlur());
    }

    // clicking the terminal moves focus to the overlay input; the input has
    // pointer-events: none so mouse events fall through to xterm
    this.on(this.container, "mouseup", () => {
      if (this.hasTerminalSelection()) {
        return;
      }
      requestAnimationFrame(() => {
        if (this.hasTerminalSelection()) {
          return;
        }
        this.input.focus();
      });
    });
  }

  /* ── Public API ───────────────────────────── */

  focus(): void {
    this.input.focus();
  }

  dispose(): void {
    this.isDisposed = true;
    this.resetImeStateOnBlur();
    this.flushPendingHangulJamo();
    this.clearPendingHangulFlushTimer();
    for (const cleanup of this.disposables) cleanup();
    this.disposables.length = 0;
    this.input.remove();
    this.preview.remove();
  }

  /* ── Util ─────────────────────────────────── */

  /**
   * Event-stream logging for IME regression diagnosis — enable from the
   * webview console with `window.__lazedImeDebug = true`.
   */
  private debugLog(...args: unknown[]): void {
    if ((window as unknown as Record<string, unknown>).__lazedImeDebug) {
      console.log("[ime]", ...args);
    }
  }

  private on<K extends keyof HTMLElementEventMap>(
    el: EventTarget,
    event: K,
    handler: (e: HTMLElementEventMap[K]) => void,
  ): void {
    el.addEventListener(event, handler as EventListener);
    this.disposables.push(() =>
      el.removeEventListener(event, handler as EventListener),
    );
  }

  private resetImeStateOnBlur(): void {
    if (this.isComposing || this.pendingHangulConsumedByComposition) {
      this.discardPendingHangulJamo();
    } else {
      this.flushPendingHangulJamo();
    }

    this.isComposing = false;
    this.suppressedPostCompositionInput = null;
    this.keydownAwaitingInput = null;
    this.swallowedKey = null;
    this.preview.style.display = "none";
    this.preview.textContent = "";
    this.input.value = "";
  }

  private writeDroppedPaths(paths: string[]): void {
    const serialized = formatDroppedPathsForTerminal(paths);
    if (!serialized) {
      return;
    }

    this.flushPendingHangulJamo();
    this.input.value = "";
    this.writeToPty(serialized);
    this.input.focus();
  }

  private containsPhysicalPosition(
    physicalX: number,
    physicalY: number,
  ): boolean {
    const scale = getDevicePixelRatio();
    const cssX = physicalX / scale;
    const cssY = physicalY / scale;
    const rect = this.container.getBoundingClientRect();
    return (
      cssX >= rect.left &&
      cssX <= rect.right &&
      cssY >= rect.top &&
      cssY <= rect.bottom
    );
  }

  private hasTerminalSelection(): boolean {
    return this.terminal.getSelection().length > 0;
  }
}

function shellSingleQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

function hasDroppedFiles(dataTransfer: DataTransfer | null): boolean {
  if (!dataTransfer) {
    return false;
  }
  if (dataTransfer.files.length > 0) {
    return true;
  }

  return Array.from(dataTransfer.items ?? []).some(
    (item) => item.kind === "file",
  );
}

function extractDroppedPaths(dataTransfer: DataTransfer | null): string[] {
  if (!dataTransfer) {
    return [];
  }

  const directPaths = Array.from(dataTransfer.files)
    .map((file) => getFilePath(file))
    .filter((value): value is string => Boolean(value));
  if (directPaths.length > 0) {
    return directPaths;
  }

  const uriList = safeGetDataTransferValue(dataTransfer, "text/uri-list");
  if (!uriList) {
    return [];
  }

  return uriList
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter((line) => line.length > 0 && !line.startsWith("#"))
    .map(parseFileUri)
    .filter((value): value is string => Boolean(value));
}

function getFilePath(file: File): string | null {
  const fileWithPath = file as File & { path?: string };
  return fileWithPath.path?.trim() || null;
}

function safeGetDataTransferValue(
  dataTransfer: DataTransfer,
  type: string,
): string {
  try {
    return dataTransfer.getData(type);
  } catch {
    return "";
  }
}

function parseFileUri(value: string): string | null {
  if (!value.startsWith("file://")) {
    return null;
  }

  try {
    const url = new URL(value);
    return decodeURIComponent(url.pathname);
  } catch {
    return null;
  }
}

function formatDroppedPathsForTerminal(paths: string[]): string {
  const serialized = paths
    .map((path) => path.trim())
    .filter((path) => path.length > 0)
    .map((path) => shellSingleQuote(path));
  if (serialized.length === 0) {
    return "";
  }
  return `${serialized.join(" ")} `;
}

function getDevicePixelRatio(): number {
  if (typeof window === "undefined") {
    return 1;
  }
  const ratio = window.devicePixelRatio;
  return Number.isFinite(ratio) && ratio > 0 ? ratio : 1;
}

function isHangulCharacter(value: string): boolean {
  if (value.length !== 1) return false;
  const code = value.charCodeAt(0);

  // Hangul Jamo / Compatibility Jamo / Syllables + extension blocks
  return (
    (code >= 0x1100 && code <= 0x11ff) ||
    (code >= 0x3130 && code <= 0x318f) ||
    (code >= 0xa960 && code <= 0xa97f) ||
    (code >= 0xac00 && code <= 0xd7a3) ||
    (code >= 0xd7b0 && code <= 0xd7ff)
  );
}

function isHangulJamoCharacter(value: string): boolean {
  if (value.length !== 1) return false;
  const code = value.charCodeAt(0);

  return (
    (code >= 0x1100 && code <= 0x11ff) ||
    (code >= 0x3130 && code <= 0x318f) ||
    (code >= 0xa960 && code <= 0xa97f) ||
    (code >= 0xd7b0 && code <= 0xd7ff)
  );
}

function containsOnlyHangulJamo(value: string): boolean {
  if (!value) return false;
  for (const char of value) {
    if (!isHangulJamoCharacter(char)) return false;
  }
  return true;
}

function containsHangul(value: string): boolean {
  for (const char of value) {
    if (isHangulCharacter(char)) return true;
  }
  return false;
}

/**
 * committed text made only of complete syllables (plus non-jamo chars like
 * spaces) — a bare jamo anywhere means the commit may still be a fragment
 * that a following keystroke could merge into
 */
function isCompleteSyllableText(value: string): boolean {
  let sawSyllable = false;
  for (const char of value) {
    if (isHangulJamoCharacter(char)) return false;
    const code = char.charCodeAt(0);
    if (code >= 0xac00 && code <= 0xd7a3) sawSyllable = true;
  }
  return sawSyllable;
}

/**
 * Idle delay before buffered jamo are committed to the PTY. Kept generous:
 * right after an input-source switch, WKWebView delivers the first
 * syllable's events with extra latency and the composition preview already
 * gives the user immediate feedback.
 */
const HANGUL_JAMO_FLUSH_DELAY_MS = 300;

/** how long a swallowed keydown stays recoverable after the switch */
const SWALLOWED_KEY_RECOVERY_MS = 800;

/** back-to-back window for treating a repeated jamo event as a resend */
const JAMO_RESEND_WINDOW_MS = 60;

/** physical key (KeyboardEvent.code) → 2-set jamo, used to recover the
 * first keystroke eaten by an input-source switch */
const KOREAN_2SET_KEYCODE: Record<string, string> = {
  KeyQ: "ㅂ",
  KeyW: "ㅈ",
  KeyE: "ㄷ",
  KeyR: "ㄱ",
  KeyT: "ㅅ",
  KeyY: "ㅛ",
  KeyU: "ㅕ",
  KeyI: "ㅑ",
  KeyO: "ㅐ",
  KeyP: "ㅔ",
  KeyA: "ㅁ",
  KeyS: "ㄴ",
  KeyD: "ㅇ",
  KeyF: "ㄹ",
  KeyG: "ㅎ",
  KeyH: "ㅗ",
  KeyJ: "ㅓ",
  KeyK: "ㅏ",
  KeyL: "ㅣ",
  KeyZ: "ㅋ",
  KeyX: "ㅌ",
  KeyC: "ㅊ",
  KeyV: "ㅍ",
  KeyB: "ㅠ",
  KeyN: "ㅜ",
  KeyM: "ㅡ",
};

const KOREAN_2SET_SHIFT_KEYCODE: Record<string, string> = {
  KeyQ: "ㅃ",
  KeyW: "ㅉ",
  KeyE: "ㄸ",
  KeyR: "ㄲ",
  KeyT: "ㅆ",
  KeyO: "ㅒ",
  KeyP: "ㅖ",
};
const HANGUL_SYLLABLE_BASE = 0xac00;
const HANGUL_SYLLABLE_V_COUNT = 21;
const HANGUL_SYLLABLE_T_COUNT = 28;

const INITIAL_COMPAT_INDEX: Record<string, number> = {
  ㄱ: 0,
  ㄲ: 1,
  ㄴ: 2,
  ㄷ: 3,
  ㄸ: 4,
  ㄹ: 5,
  ㅁ: 6,
  ㅂ: 7,
  ㅃ: 8,
  ㅅ: 9,
  ㅆ: 10,
  ㅇ: 11,
  ㅈ: 12,
  ㅉ: 13,
  ㅊ: 14,
  ㅋ: 15,
  ㅌ: 16,
  ㅍ: 17,
  ㅎ: 18,
};

const VOWEL_COMPAT_INDEX: Record<string, number> = {
  ㅏ: 0,
  ㅐ: 1,
  ㅑ: 2,
  ㅒ: 3,
  ㅓ: 4,
  ㅔ: 5,
  ㅕ: 6,
  ㅖ: 7,
  ㅗ: 8,
  ㅘ: 9,
  ㅙ: 10,
  ㅚ: 11,
  ㅛ: 12,
  ㅜ: 13,
  ㅝ: 14,
  ㅞ: 15,
  ㅟ: 16,
  ㅠ: 17,
  ㅡ: 18,
  ㅢ: 19,
  ㅣ: 20,
};

const FINAL_COMPAT_INDEX: Record<string, number> = {
  ㄱ: 1,
  ㄲ: 2,
  ㄳ: 3,
  ㄴ: 4,
  ㄵ: 5,
  ㄶ: 6,
  ㄷ: 7,
  ㄹ: 8,
  ㄺ: 9,
  ㄻ: 10,
  ㄼ: 11,
  ㄽ: 12,
  ㄾ: 13,
  ㄿ: 14,
  ㅀ: 15,
  ㅁ: 16,
  ㅂ: 17,
  ㅄ: 18,
  ㅅ: 19,
  ㅆ: 20,
  ㅇ: 21,
  ㅈ: 22,
  ㅊ: 23,
  ㅋ: 24,
  ㅌ: 25,
  ㅍ: 26,
  ㅎ: 27,
};

const COMBINED_VOWELS: Record<string, string> = {
  ㅗㅏ: "ㅘ",
  ㅗㅐ: "ㅙ",
  ㅗㅣ: "ㅚ",
  ㅜㅓ: "ㅝ",
  ㅜㅔ: "ㅞ",
  ㅜㅣ: "ㅟ",
  ㅡㅣ: "ㅢ",
};

const COMBINED_FINALS: Record<string, string> = {
  ㄱㅅ: "ㄳ",
  ㄴㅈ: "ㄵ",
  ㄴㅎ: "ㄶ",
  ㄹㄱ: "ㄺ",
  ㄹㅁ: "ㄻ",
  ㄹㅂ: "ㄼ",
  ㄹㅅ: "ㄽ",
  ㄹㅌ: "ㄾ",
  ㄹㅍ: "ㄿ",
  ㄹㅎ: "ㅀ",
  ㅂㅅ: "ㅄ",
};

function hasOwn(map: Record<string, number>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(map, key);
}

function isCompatConsonant(value: string): boolean {
  return (
    hasOwn(INITIAL_COMPAT_INDEX, value) || hasOwn(FINAL_COMPAT_INDEX, value)
  );
}

function isCompatVowel(value: string): boolean {
  return hasOwn(VOWEL_COMPAT_INDEX, value);
}

function combineCompatVowel(first: string, second: string): string | null {
  return COMBINED_VOWELS[`${first}${second}`] ?? null;
}

function combineCompatFinal(first: string, second: string): string | null {
  return COMBINED_FINALS[`${first}${second}`] ?? null;
}

function reverseIndex(map: Record<string, number>, size: number): string[] {
  const list = new Array<string>(size).fill("");
  for (const [char, index] of Object.entries(map)) list[index] = char;
  return list;
}

const INITIAL_LIST = reverseIndex(INITIAL_COMPAT_INDEX, 19);
const VOWEL_LIST = reverseIndex(VOWEL_COMPAT_INDEX, 21);
const FINAL_LIST = reverseIndex(FINAL_COMPAT_INDEX, 28);

function decomposeHangulChar(char: string): string {
  const code = char.charCodeAt(0);
  if (code < 0xac00 || code > 0xd7a3) return char;
  const offset = code - 0xac00;
  const initial = INITIAL_LIST[Math.floor(offset / 588)];
  const vowel = VOWEL_LIST[Math.floor((offset % 588) / 28)];
  const final = FINAL_LIST[offset % 28];
  return `${initial}${vowel}${final}`;
}

/** compat-jamo text of a value — syllables decompose, other chars pass */
function jamoText(value: string): string {
  return [...value].map(decomposeHangulChar).join("");
}

function composeCompatSyllable(
  initial: string,
  vowel: string,
  final: string,
): string | null {
  const initialIndex = INITIAL_COMPAT_INDEX[initial];
  const vowelIndex = VOWEL_COMPAT_INDEX[vowel];
  if (initialIndex === undefined || vowelIndex === undefined) return null;

  const finalIndex = final ? FINAL_COMPAT_INDEX[final] : 0;
  if (final && finalIndex === undefined) return null;

  const syllableCode =
    HANGUL_SYLLABLE_BASE +
    (initialIndex * HANGUL_SYLLABLE_V_COUNT + vowelIndex) *
      HANGUL_SYLLABLE_T_COUNT +
    finalIndex;
  return String.fromCharCode(syllableCode);
}

interface ParsedHangulUnit {
  /** chars consumed by this unit */
  length: number;
  vowel: string;
  final: string;
}

/**
 * Parse one in-progress syllable unit starting at chars[start]:
 * initial + vowel [+ final [+ final]].
 * Returns null when chars[start] can't open a syllable.
 */
function parseHangulUnit(
  chars: string[],
  start: number,
): ParsedHangulUnit | null {
  const initial = chars[start];
  if (!hasOwn(INITIAL_COMPAT_INDEX, initial)) return null;

  const next = chars[start + 1];
  if (!next || !isCompatVowel(next)) return null;

  let length = 2;
  let vowel = next;
  const nextVowel = chars[start + length];
  if (nextVowel && isCompatVowel(nextVowel)) {
    const combined = combineCompatVowel(vowel, nextVowel);
    if (combined) {
      vowel = combined;
      length += 1;
    }
  }

  let finalConsonant = "";
  const finalCandidate = chars[start + length];
  if (finalCandidate && isCompatConsonant(finalCandidate)) {
    const afterFinal = chars[start + length + 1];
    if (!afterFinal || !isCompatVowel(afterFinal)) {
      let finalValue = finalCandidate;
      let finalLength = 1;

      if (isCompatConsonant(afterFinal)) {
        const combinedFinal = combineCompatFinal(finalCandidate, afterFinal);
        const afterCombinedFinal = chars[start + length + 2];
        if (
          combinedFinal &&
          (!afterCombinedFinal || !isCompatVowel(afterCombinedFinal))
        ) {
          finalValue = combinedFinal;
          finalLength = 2;
        }
      }

      if (hasOwn(FINAL_COMPAT_INDEX, finalValue)) {
        finalConsonant = finalValue;
        length += finalLength;
      }
    }
  }

  return { length, vowel, final: finalConsonant };
}

/** start index of the trailing in-progress syllable unit */
function lastComposingUnitStart(chars: string[]): number {
  let last = 0;
  let i = 0;
  while (i < chars.length) {
    last = i;
    i += parseHangulUnit(chars, i)?.length ?? 1;
  }
  return last;
}

function composeHangulJamo(value: string): string {
  const chars = [...value];
  let result = "";
  let i = 0;

  while (i < chars.length) {
    const current = chars[i];
    const unit = parseHangulUnit(chars, i);
    if (!unit) {
      result += current;
      i += 1;
      continue;
    }

    const composed = composeCompatSyllable(current, unit.vowel, unit.final);
    if (!composed) {
      result += current;
      i += 1;
      continue;
    }

    result += composed;
    i += unit.length;
  }

  return result;
}
