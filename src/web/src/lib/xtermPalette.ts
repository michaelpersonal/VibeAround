/**
 * xterm.js color palettes for dark and light app themes.
 *
 * Palette colors (black, red, green … brightWhite) are fixed per theme mode.
 * The terminal background, cursor, and selection colors come from the
 * per-tool ToolTheme and are injected by buildXtermTheme().
 */

import type { ToolTheme } from "./terminal-types";

/** Shared shape for the static palette portion (no background/cursor/selection). */
interface XtermPalette {
  foreground: string;
  black: string;       red: string;       green: string;    yellow: string;
  blue: string;        magenta: string;   cyan: string;     white: string;
  brightBlack: string; brightRed: string; brightGreen: string; brightYellow: string;
  brightBlue: string;  brightMagenta: string; brightCyan: string; brightWhite: string;
}

const XTERM_PALETTE_DARK: XtermPalette = {
  foreground:    "#c8c8d8",
  black:         "#1a1a2e", red:           "#f87171", green:        "#4ade80", yellow:       "#fbbf24",
  blue:          "#60a5fa", magenta:       "#c084fc", cyan:         "#4fd1c5", white:        "#c8c8d8",
  brightBlack:   "#4a4a6a", brightRed:     "#fca5a5", brightGreen:  "#86efac", brightYellow: "#fde68a",
  brightBlue:    "#93c5fd", brightMagenta: "#d8b4fe", brightCyan:   "#5eead4", brightWhite:  "#f0f0f8",
};

const XTERM_PALETTE_LIGHT: XtermPalette = {
  foreground:    "#1e293b",
  black:         "#475569", red:           "#dc2626", green:        "#16a34a", yellow:       "#ca8a04",
  blue:          "#2563eb", magenta:       "#9333ea", cyan:         "#0891b2", white:        "#64748b",
  brightBlack:   "#94a3b8", brightRed:     "#ef4444", brightGreen:  "#22c55e", brightYellow: "#eab308",
  brightBlue:    "#3b82f6", brightMagenta: "#a855f7", brightCyan:   "#06b6d4", brightWhite:  "#f1f5f9",
};

/**
 * Build a full xterm ITheme by merging the static palette with the
 * per-tool colors (background, cursor, selection).
 */
export function buildXtermTheme(tool: ToolTheme, isDark: boolean) {
  const palette = isDark ? XTERM_PALETTE_DARK : XTERM_PALETTE_LIGHT;
  return {
    ...palette,
    background:          tool.bg,
    cursor:              tool.cursorColor,
    cursorAccent:        tool.bg,
    selectionBackground: tool.selectionBg,
    selectionForeground: isDark ? "#ffffff" : "#0f172a",
  };
}
