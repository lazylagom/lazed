import { useCallback, useEffect, useRef, useState } from "react";
import { SIDEBAR_RESET_EVENT } from "../shared/settings";

const SIDE_MIN = 180;
const SIDE_MAX = 560;
const SIDE_DEFAULT = 288;
const SIDE_KEY = "sidebar-width";

function loadSideWidth() {
  const v = Number(localStorage.getItem(SIDE_KEY));
  if (!Number.isFinite(v) || v <= 0) return SIDE_DEFAULT;
  return Math.min(SIDE_MAX, Math.max(SIDE_MIN, v));
}

/** The sidebar-space width is shared by every rail view (projects, inbox…):
 * one localStorage key, one drag handle contract, one reset event. */
export function useSidebarWidth() {
  const [width, setWidth] = useState(loadSideWidth);
  const sideRef = useRef<HTMLDivElement>(null);

  const onResizeDown = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = sideRef.current?.getBoundingClientRect().width ?? width;
    const clamp = (v: number) => Math.min(SIDE_MAX, Math.max(SIDE_MIN, v));
    const onMove = (ev: MouseEvent) =>
      setWidth(clamp(startW + ev.clientX - startX));
    const onUp = (ev: MouseEvent) => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      const w = Math.round(clamp(startW + ev.clientX - startX));
      localStorage.setItem(SIDE_KEY, String(w));
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const resetWidth = useCallback(() => {
    setWidth(SIDE_DEFAULT);
    localStorage.setItem(SIDE_KEY, String(SIDE_DEFAULT));
  }, []);

  useEffect(() => {
    window.addEventListener(SIDEBAR_RESET_EVENT, resetWidth);
    return () => window.removeEventListener(SIDEBAR_RESET_EVENT, resetWidth);
  }, [resetWidth]);

  return { width, sideRef, onResizeDown, resetWidth };
}
