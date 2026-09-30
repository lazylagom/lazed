/** Serialize snapshots and reject responses predating an event or a new
 * explicit refresh. All callers await the final, current snapshot. */
export function createSnapshotLoader<T>(
  read: () => Promise<T>,
  publish: (value: T) => void,
) {
  let revision = 0;
  let flight: Promise<T> | null = null;
  const invalidate = () => {
    revision += 1;
  };
  const load = (): Promise<T> => {
    invalidate();
    if (flight) return flight;
    const request = (async () => {
      for (;;) {
        const started = revision;
        let value: T;
        try {
          value = await read();
        } catch (error) {
          if (started !== revision) continue;
          flight = null;
          throw error;
        }
        if (started !== revision) continue;
        // The read has finished. A refresh dispatched by publication must
        // start a new read rather than join this already settled response.
        flight = null;
        publish(value);
        return value;
      }
    })().finally(() => {
      if (flight === request) flight = null;
    });
    flight = request;
    return request;
  };
  return { load, invalidate };
}
