import { createSignal, onCleanup, onMount } from "solid-js";

import { usePlatform } from "@/platform/context";
import { DPI_TEST_FINISHED_EVENT, DPI_TEST_PROGRESS_EVENT } from "@/types";
import type { DpiTestProgress, DpiTestResult, DpiTestStatus } from "@/types";

/**
 * The DPI strategy test (Settings → DPI): every stored strategy is run
 * against every configured test site. The backend persists per-strategy
 * pass rates and emits `dpi-test-progress` per completed strategy — this
 * hook drives the button (busy state, live progress, cancellation) and
 * forwards each result to `onResult` so the strategy list can update in
 * place. A test started earlier (the user left the section and came back)
 * is adopted on mount via `dpi_test_status` and released by the final
 * `dpi-test-finished` event.
 */
export function useDpiTest(onResult: (result: DpiTestResult) => void) {
  const platform = usePlatform();
  const [testing, setTesting] = createSignal(false);
  const [stopping, setStopping] = createSignal(false);
  const [progress, setProgress] = createSignal<DpiTestProgress | null>(null);
  const [error, setError] = createSignal<string | null>(null);

  const release = () => {
    setTesting(false);
    setStopping(false);
    setProgress(null);
  };

  onMount(() => {
    let disposed = false;
    const keep = (promise: Promise<() => void>) => {
      void promise.then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          onCleanup(() => unlisten());
        }
      });
    };

    // adopt a test that is already running in the backend
    void platform.invoke<DpiTestStatus | null>("dpi_test_status").then(
      (status) => {
        if (disposed || status === null) {
          return;
        }
        setTesting(true);
        setProgress({ done: status.done, total: status.total, results: [] });
      },
      (cause) => setError(cause instanceof Error ? cause.message : String(cause)),
    );

    keep(
      platform.listen<DpiTestProgress>(DPI_TEST_PROGRESS_EVENT, (payload) => {
        setTesting(true);
        setProgress(payload);
        for (const result of payload.results) {
          onResult(result);
        }
      }),
    );

    // the test is over (finished, cancelled or failed) — release the button
    keep(platform.listen(DPI_TEST_FINISHED_EVENT, release));

    onCleanup(() => {
      disposed = true;
    });
  });

  const test = async () => {
    if (testing()) {
      return;
    }
    setError(null);
    setTesting(true);
    setStopping(false);
    setProgress(null);
    try {
      await platform.invoke("dpi_test_strategies");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      release();
    }
  };

  /** Stops the running test after the current strategy; the button is
   *  released by the finishing command (own `test`) or by the final
   *  `dpi-test-finished` event (adopted tests). */
  const cancel = async () => {
    if (!testing() || stopping()) {
      return;
    }
    setStopping(true);
    try {
      await platform.invoke("dpi_cancel_test");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setStopping(false);
    }
  };

  return { testing, stopping, progress, error, test, cancel };
}
