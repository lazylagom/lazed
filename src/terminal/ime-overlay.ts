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
      const committed = e.data ?? "";
      const { text: pending, consumedByComposition } =
        this.takePendingHangulJamo();
      this.isComposing = false;
      this.preview.style.display = "none";
      this.preview.textContent = "";

      if (!consumedByComposition && pending && pending !== committed) {
        this.writeToPty(pending);
      }

      if (committed) {
        this.writeToPty(committed);
        this.suppressedPostCompositionInput = committed;
      }
      this.input.value = "";
    });
  }

  /* ── Keyboard — non-IME key handling ──────── */

  private setupKeyboardEvents(): void {
    this.on(this.input, "keydown", (e: KeyboardEvent) => {
      // never interfere while the IME is composing
      if (this.isComposing || e.isComposing || e.keyCode === 229) return;

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
      const isCompositionInput =
        inputEvent.isComposing ||
        inputType === "insertCompositionText" ||
        inputType === "insertFromComposition";

      if (this.isComposing || isCompositionInput || !this.input.value) return;

      if (this.shouldSuppressPostCompositionInput(this.input.value)) {
        this.input.value = "";
        return;
      }

      if (containsOnlyHangulJamo(this.input.value)) {
        this.queuePendingHangulJamo(this.input.value);
        this.input.value = "";
        return;
      }

      this.flushPendingHangulJamo();
      this.writeToPty(this.input.value);
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

    if (!this.pendingHangulJamo) {
      this.pendingHangulJamo = value;
      this.pendingHangulConsumedByComposition = false;
    } else if (value.length > 1 && value.startsWith(this.pendingHangulJamo)) {
      // some input methods resend the accumulated string
      this.pendingHangulJamo = value;
    } else if (!this.pendingHangulJamo.endsWith(value)) {
      this.pendingHangulJamo += value;
    }

    this.schedulePendingHangulFlush();
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
    return { text, consumedByComposition };
  }

  private discardPendingHangulJamo(): void {
    this.clearPendingHangulFlushTimer();
    this.pendingHangulJamo = "";
    this.pendingHangulConsumedByComposition = false;
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

const HANGUL_JAMO_FLUSH_DELAY_MS = 80;
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

function composeHangulJamo(value: string): string {
  const chars = [...value];
  let result = "";
  let i = 0;

  while (i < chars.length) {
    const current = chars[i];
    if (!hasOwn(INITIAL_COMPAT_INDEX, current)) {
      result += current;
      i += 1;
      continue;
    }

    const next = chars[i + 1];
    if (!next || !isCompatVowel(next)) {
      result += current;
      i += 1;
      continue;
    }

    let consumed = 2;
    let vowel = next;
    const nextVowel = chars[i + consumed];
    if (nextVowel && isCompatVowel(nextVowel)) {
      const combined = combineCompatVowel(vowel, nextVowel);
      if (combined) {
        vowel = combined;
        consumed += 1;
      }
    }

    let finalConsonant = "";
    const finalCandidate = chars[i + consumed];
    if (finalCandidate && isCompatConsonant(finalCandidate)) {
      const afterFinal = chars[i + consumed + 1];
      if (!afterFinal || !isCompatVowel(afterFinal)) {
        let finalValue = finalCandidate;
        let finalConsumed = 1;

        const secondFinalCandidate = chars[i + consumed + 1];
        const afterCombinedFinal = chars[i + consumed + 2];
        if (secondFinalCandidate && isCompatConsonant(secondFinalCandidate)) {
          const combinedFinal = combineCompatFinal(
            finalCandidate,
            secondFinalCandidate,
          );
          if (
            combinedFinal &&
            (!afterCombinedFinal || !isCompatVowel(afterCombinedFinal))
          ) {
            finalValue = combinedFinal;
            finalConsumed = 2;
          }
        }

        if (hasOwn(FINAL_COMPAT_INDEX, finalValue)) {
          finalConsonant = finalValue;
          consumed += finalConsumed;
        }
      }
    }

    const composed = composeCompatSyllable(current, vowel, finalConsonant);
    if (!composed) {
      result += current;
      i += 1;
      continue;
    }

    result += composed;
    i += consumed;
  }

  return result;
}
