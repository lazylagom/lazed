import { useCallback, useEffect, useRef, useState } from "react";
import { SIDEBAR_RESET_EVENT } from "../shared/settings";

const SIDE_MIN = 180;
const SIDE_MAX = 560;
const SIDE_DEFAULT = 288;
const SIDE_KEY = "sidebar-width";

interface PanelWidthOpts {
  storageKey?: string;
  /** which window edge the panel sits on — right-side handles invert drag delta */
  side?: "left" | "right";
  defaultWidth?: number;
}

function loadSideWidth(key: string, fallback: number) {
  const v = Number(localStorage.getItem(key));
  if (!Number.isFinite(v) || v <= 0) return fallback;
  return Math.min(SIDE_MAX, Math.max(SIDE_MIN, v));
}

/** Resizable panel width — one drag-handle contract shared by the left
 * sidebar and the right files panel; each keeps its own localStorage key.
 * Dragging the right panel's left edge inverts the delta. */
export function useSidebarWidth({
  storageKey = SIDE_KEY,
  side = "left",
  defaultWidth = SIDE_DEFAULT,
}: PanelWidthOpts = {}) {
  const [width, setWidth] = useState(() =>
    loadSideWidth(storageKey, defaultWidth),
  );
  const sideRef = useRef<HTMLDivElement>(null);

  const onResizeDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = sideRef.current?.getBoundingClientRect().width ?? width;
    const sign = side === "right" ? -1 : 1;
    const clamp = (v: number) => Math.min(SIDE_MAX, Math.max(SIDE_MIN, v));
    const onMove = (ev: MouseEvent) =>
      setWidth(clamp(startW + sign * (ev.clientX - startX)));
    const onUp = (ev: MouseEvent) => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      const w = Math.round(clamp(startW + sign * (ev.clientX - startX)));
      localStorage.setItem(storageKey, String(w));
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const resetWidth = useCallback(() => {
    setWidth(defaultWidth);
    localStorage.setItem(storageKey, String(defaultWidth));
  }, [storageKey, defaultWidth]);

  useEffect(() => {
    window.addEventListener(SIDEBAR_RESET_EVENT, resetWidth);
    return () => window.removeEventListener(SIDEBAR_RESET_EVENT, resetWidth);
  }, [resetWidth]);

  return { width, sideRef, onResizeDown, resetWidth };
}
