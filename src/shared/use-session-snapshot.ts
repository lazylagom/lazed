import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type Snapshot, lazed, normalize } from "./lazed";
import { createSnapshotLoader } from "./snapshot-loader";

export function useSessionSnapshot() {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const mounted = useRef(true);
  const loader = useMemo(
    () =>
      createSnapshotLoader(lazed.snapshot, (value) => {
        if (mounted.current) setSnap(normalize(value));
      }),
    [],
  );
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      loader.invalidate();
    };
  }, [loader]);
  const patch = useCallback(
    (update: (previous: Snapshot | null) => Snapshot | null) => {
      loader.invalidate();
      setSnap(update);
    },
    [loader],
  );
  return {
    snap,
    patch,
    loadSnapshot: loader.load,
    invalidateSnapshot: loader.invalidate,
  };
}
