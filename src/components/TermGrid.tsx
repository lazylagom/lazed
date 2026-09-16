import type { TerminalInfo } from "../shared/lazed";
import { TermView } from "./TermView";

/** Equal-width flex row of a project's terminals. Commander first, then
 *  worktrees, then plain terminals (order matches project.terminals). */
export function TermGrid({
  terms,
  focusedTerm,
  onFocusTerm,
  onCloseTerm,
}: {
  terms: TerminalInfo[];
  focusedTerm: string | null;
  onFocusTerm: (id: string) => void;
  onCloseTerm: (id: string) => void;
}) {
  return (
    <div className="pane-grid term-grid">
      {terms.map((t) => (
        <div key={t.term_id} className="term-cell-wrap">
          <TermView
            term={t}
            focused={t.term_id === focusedTerm}
            onFocus={() => onFocusTerm(t.term_id)}
            onClose={onCloseTerm}
          />
        </div>
      ))}
    </div>
  );
}
