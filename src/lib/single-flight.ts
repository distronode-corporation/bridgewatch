/**
 * The guard that makes a write go out once however fast it is clicked: the
 * job view's Retry and Play, and the token form's credential-store writes.
 */

/**
 * One request per key at a time.
 *
 * ⛔ A ref, not the button's `disabled`: two clicks can both land before
 * Svelte has re-rendered the first one's disabled state, and both would then
 * go out. The set is checked and filled synchronously, before the first
 * `await`, so the second click finds the key taken and sends nothing.
 */
export function createSingleFlight() {
  const busy = new Set<string>();
  return {
    /** Run `task` unless `key` is already running; `null` when it was. */
    run<T>(key: string, task: () => Promise<T>): Promise<T> | null {
      if (busy.has(key)) return null;
      busy.add(key);
      let pending: Promise<T>;
      try {
        pending = task();
      } catch (error) {
        busy.delete(key);
        return Promise.reject(error);
      }
      return pending.finally(() => busy.delete(key));
    },
    /** Whether `key` is running now. */
    busy(key: string): boolean {
      return busy.has(key);
    },
  };
}
