import { useCallback, useRef } from "react";
import type { LayoutSplit, PaneInfo, TabLayout } from "../shared/herdr";
import { PaneView } from "./PaneView";

interface Divider {
  split: LayoutSplit;
  /** pane adjacent on the near side (left/top) of the divider */
  nearPane?: string;
  /** pane adjacent on the far side (right/bottom) */
  farPane?: string;
  style: React.CSSProperties;
  axis: "x" | "y";
}

export function PaneGrid({
  layout,
  panesById,
  focusedPane,
  onFocusPane,
  onClosePane,
  onResize,
}: {
  layout: TabLayout;
  panesById: Map<string, PaneInfo>;
  focusedPane: string | null;
  onFocusPane: (id: string) => void;
  onClosePane: (id: string) => void;
  onResize: (paneId: string, direction: string, amount: number) => void;
}) {
  const gridRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    div: Divider;
    last: number;
    lastSent: number;
  } | null>(null);
  const { area } = layout;

  const pct = {
    x: (v: number) => `${(v / area.width) * 100}%`,
    y: (v: number) => `${(v / area.height) * 100}%`,
  };

  const dividers: Divider[] = layout.splits
    .map((split) => {
      const r = split.rect;
      if (split.direction === "right") {
        const dx = r.x + r.width * split.ratio;
        const near = layout.panes.find(
          (p) =>
            Math.abs(p.rect.x + p.rect.width - dx) < 0.6 &&
            p.rect.y < r.y + r.height &&
            p.rect.y + p.rect.height > r.y,
        );
        const far = layout.panes.find(
          (p) =>
            Math.abs(p.rect.x - dx) < 0.6 &&
            p.rect.y < r.y + r.height &&
            p.rect.y + p.rect.height > r.y,
        );
        return {
          split,
          nearPane: near?.pane_id,
          farPane: far?.pane_id,
          axis: "x" as const,
          style: {
            left: `calc(${pct.x(dx)} - 4px)`,
            top: pct.y(r.y),
            width: 8,
            height: pct.y(r.height),
          },
        };
      }
      const dy = r.y + r.height * split.ratio;
      const near = layout.panes.find(
        (p) =>
          Math.abs(p.rect.y + p.rect.height - dy) < 0.6 &&
          p.rect.x < r.x + r.width &&
          p.rect.x + p.rect.width > r.x,
      );
      const far = layout.panes.find(
        (p) =>
          Math.abs(p.rect.y - dy) < 0.6 &&
          p.rect.x < r.x + r.width &&
          p.rect.x + p.rect.width > r.x,
      );
      return {
        split,
        nearPane: near?.pane_id,
        farPane: far?.pane_id,
        axis: "y" as const,
        style: {
          left: pct.x(r.x),
          top: `calc(${pct.y(dy)} - 4px)`,
          width: pct.x(r.width),
          height: 8,
        },
      };
    })
    .filter((d) => d.nearPane || d.farPane);

  const onDividerDown = useCallback(
    (div: Divider) => (e: React.MouseEvent) => {
      e.preventDefault();
      const pos = div.axis === "x" ? e.clientX : e.clientY;
      dragRef.current = { div, last: pos, lastSent: pos };

      const onMove = (ev: MouseEvent) => {
        const drag = dragRef.current;
        if (!drag) return;
        const cur = drag.div.axis === "x" ? ev.clientX : ev.clientY;
        drag.last = cur;
      };
      const onUp = () => {
        const drag = dragRef.current;
        dragRef.current = null;
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        if (!drag) return;
        const grid = gridRef.current;
        if (!grid) return;
        const deltaPx = drag.last - drag.lastSent;
        const rect = drag.div.split.rect;
        const gridRect = grid.getBoundingClientRect();
        const splitPx =
          drag.div.axis === "x"
            ? (rect.width / area.width) * gridRect.width
            : (rect.height / area.height) * gridRect.height;
        if (splitPx <= 0) return;
        const frac = deltaPx / splitPx;
        if (Math.abs(frac) < 0.005) return;
        const growDir =
          drag.div.axis === "x" ? ["right", "left"] : ["down", "up"];
        if (frac > 0 && drag.div.nearPane) {
          onResize(drag.div.nearPane, growDir[0], frac);
        } else if (frac < 0 && drag.div.farPane) {
          onResize(drag.div.farPane, growDir[1], -frac);
        }
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [area.width, area.height, onResize],
  );

  return (
    <div ref={gridRef} className="pane-grid">
      {layout.panes.map((lp) => {
        const info = panesById.get(lp.pane_id);
        if (!info) return null;
        const r = lp.rect;
        return (
          <div
            key={lp.pane_id}
            className="pane-abs"
            style={{
              left: pct.x(r.x),
              top: pct.y(r.y),
              width: pct.x(r.width),
              height: pct.y(r.height),
            }}
          >
            <PaneView
              pane={info}
              focused={lp.pane_id === focusedPane}
              onFocus={() => onFocusPane(lp.pane_id)}
              onClose={onClosePane}
            />
          </div>
        );
      })}
      {dividers.map((d) => (
        <div
          key={d.split.id}
          className={`divider ${d.axis}`}
          style={d.style}
          onMouseDown={onDividerDown(d)}
        />
      ))}
    </div>
  );
}
