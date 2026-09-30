import { type ComponentType, Suspense, lazy } from "react";

/** Suspend only the requested screen, preserving the mounted terminals. */
export function deferred<P extends object>(
  load: () => Promise<{ default: ComponentType<P> }>,
) {
  const Component = lazy(load);
  return function Deferred(props: P) {
    return (
      <Suspense fallback={<div className="inbox-empty">loading…</div>}>
        <Component {...props} />
      </Suspense>
    );
  };
}
