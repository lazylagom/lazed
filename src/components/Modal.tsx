import {
  type KeyboardEventHandler,
  type ReactNode,
  type RefObject,
  useEffect,
} from "react";

/** Shared shell; each feature owns its keyboard actions and form state. */
export function Modal({
  children,
  onClose,
  onKeyDown,
  className,
  initialFocusRef,
}: {
  children: ReactNode;
  onClose: () => void;
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>;
  className?: string;
  initialFocusRef?: RefObject<HTMLInputElement | null>;
}) {
  useEffect(() => {
    initialFocusRef?.current?.focus();
  }, [initialFocusRef]);

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div
        className={className ? `modal ${className}` : "modal"}
        onMouseDown={(event) => event.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        {children}
      </div>
    </div>
  );
}
