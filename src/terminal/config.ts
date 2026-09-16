import type { ITheme } from "@xterm/xterm";

/**
 * Terminal display config — kept identical to laze
 * (src/entities/terminal/model/config.ts).
 *
 * NOTE: do not add "SF Mono" to the font chain — it breaks Hangul
 * composition. See laze docs/korean-ime.md.
 */

/** Orca terminal colors — Ghostty ANSI palette on Orca's pane surface. */
export const TERMINAL_THEME: ITheme = {
  /* keep in sync with --bg-term in src/styles.css */
  background: "#0e1014",
  foreground: "#ffffff",
  cursor: "#ffffff",
  cursorAccent: "#0e1014",
  selectionBackground: "#5a7898",
  selectionForeground: "#ffffff",
  black: "#1d1f21",
  red: "#cc6666",
  green: "#b5bd68",
  yellow: "#f0c674",
  blue: "#81a2be",
  magenta: "#b294bb",
  cyan: "#8abeb7",
  white: "#c5c8c6",
  brightBlack: "#666666",
  brightRed: "#d54e53",
  brightGreen: "#b9ca4a",
  brightYellow: "#e7c547",
  brightBlue: "#7aa6da",
  brightMagenta: "#c397d8",
  brightCyan: "#70c0b1",
  brightWhite: "#eaeaea",
};

export const TERMINAL_FONT_FAMILY = [
  "'Cascadia Code'",
  "'CaskaydiaCove Nerd Font'",
  "'MesloLGS NF'",
  "'JetBrainsMono Nerd Font'",
  "'Hack Nerd Font Mono'",
  "'Symbols Nerd Font Mono'",
  "Consolas",
  "'Courier New'",
  "Menlo",
  "monospace",
].join(", ");
