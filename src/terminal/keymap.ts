/**
 * keymap — translates keyboard events into terminal escape sequences.
 *
 * Pure functions with no terminal dependency; used by the IME overlay to
 * forward non-IME keys to the PTY. Ported from laze
 * (src/entities/terminal/model/keymap.ts).
 */

/**
 * Translate a KeyboardEvent to a terminal escape sequence.
 *
 * @param e - keyboard event
 * @param appCursor - whether Application Cursor Keys Mode (DECCKM) is active
 * @returns escape sequence string, or null when the key is unhandled
 */
export function translateKeyEvent(
  e: KeyboardEvent,
  appCursor: boolean,
): string | null {
  // Ctrl combos
  if (e.ctrlKey && !e.altKey && !e.metaKey) {
    return translateCtrl(normalizeCtrlKey(e));
  }

  // Alt(Option) combos
  // - ASCII letters: readline shortcuts (Alt+B, Alt+F, ...)
  // - symbols/digits: may be character input on international layouts — send as-is
  if (e.altKey && !e.ctrlKey && !e.metaKey && e.key.length === 1) {
    if (!isAsciiLetter(e.key)) {
      return e.key;
    }
    return `\x1b${e.key}`;
  }

  // special keys (Enter, Backspace, arrows, function keys, ...)
  const special = translateSpecialKey(e.key, e.shiftKey, appCursor);
  if (special !== null) return special;

  // plain characters
  if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
    return e.key;
  }

  return null;
}

function isAsciiLetter(value: string): boolean {
  if (value.length !== 1) return false;
  const code = value.charCodeAt(0);
  return (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
}

function normalizeCtrlKey(e: KeyboardEvent): string {
  const code = e.code ?? "";

  switch (code) {
    case "Space":
      return " ";
    case "BracketLeft":
      return "[";
    case "Backslash":
      return "\\";
    case "BracketRight":
      return "]";
    case "Digit6":
      return "6";
    case "Minus":
      return "-";
    default:
      break;
  }

  if (code.startsWith("Key") && code.length === 4) {
    return code.slice(3).toLowerCase();
  }

  return e.key;
}

/** Ctrl + key -> terminal control character (0x00-0x1F) */
export function translateCtrl(key: string): string | null {
  if (key.length === 1) {
    const code = key.toUpperCase().charCodeAt(0);
    // Ctrl+A(0x01) .. Ctrl+Z(0x1A)
    if (code >= 65 && code <= 90) return String.fromCharCode(code - 64);
  }

  switch (key) {
    case " ":
      return "\x00"; // Ctrl+Space = NUL
    case "[":
      return "\x1b"; // Ctrl+[ = ESC
    case "\\":
      return "\x1c";
    case "]":
      return "\x1d";
    case "^":
    case "6":
      return "\x1e";
    case "_":
    case "-":
      return "\x1f";
  }

  return null;
}

/** special key -> VT100/xterm escape sequence */
export function translateSpecialKey(
  key: string,
  shiftKey: boolean,
  appCursor: boolean,
): string | null {
  const csiOrSs3 = appCursor ? "\x1bO" : "\x1b[";

  switch (key) {
    case "Enter":
      // some CLI/TUI apps can't parse the tmux extkeys CSI u sequence and
      // render it literally ("[13;2u"); send LF for Shift+Enter instead
      return shiftKey ? "\n" : "\r";
    case "Backspace":
      return "\x7f";
    case "Tab":
      return shiftKey ? "\x1b[Z" : "\t";
    case "Escape":
      return "\x1b";

    // arrows
    case "ArrowUp":
      return `${csiOrSs3}A`;
    case "ArrowDown":
      return `${csiOrSs3}B`;
    case "ArrowRight":
      return `${csiOrSs3}C`;
    case "ArrowLeft":
      return `${csiOrSs3}D`;

    // navigation
    case "Home":
      return "\x1b[H";
    case "End":
      return "\x1b[F";
    case "Delete":
      return "\x1b[3~";
    case "PageUp":
      return "\x1b[5~";
    case "PageDown":
      return "\x1b[6~";
    case "Insert":
      return "\x1b[2~";

    // function keys
    case "F1":
      return "\x1bOP";
    case "F2":
      return "\x1bOQ";
    case "F3":
      return "\x1bOR";
    case "F4":
      return "\x1bOS";
    case "F5":
      return "\x1b[15~";
    case "F6":
      return "\x1b[17~";
    case "F7":
      return "\x1b[18~";
    case "F8":
      return "\x1b[19~";
    case "F9":
      return "\x1b[20~";
    case "F10":
      return "\x1b[21~";
    case "F11":
      return "\x1b[23~";
    case "F12":
      return "\x1b[24~";
  }

  return null;
}
