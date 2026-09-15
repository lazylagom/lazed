/**
 * Terminal display config — kept identical to laze
 * (src/entities/terminal/model/config.ts).
 *
 * NOTE: do not add "SF Mono" to the font chain — it breaks Hangul
 * composition. See laze docs/korean-ime.md.
 */
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
