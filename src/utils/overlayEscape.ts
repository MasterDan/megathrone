/**
 * Escape-key stack for overlays (Modal, Popover).
 *
 * Each open overlay registers its close action here while visible. A single
 * window-level keydown listener triggers only the topmost action, so one
 * Escape press closes exactly one layer: a Popover inside a Modal closes the
 * popover first, and only the next press closes the modal. Independent
 * window listeners can't do this — they all fire on the same event
 * regardless of registration order.
 */
type CloseAction = () => void;

const stack: CloseAction[] = [];

if (typeof window !== "undefined") {
  window.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") {
      return;
    }
    stack[stack.length - 1]?.();
  });
}

/** Registers a close action as the topmost overlay layer. Returns an unregister fn. */
export function registerOverlayEscape(close: CloseAction): () => void {
  stack.push(close);
  return () => {
    const index = stack.lastIndexOf(close);
    if (index >= 0) {
      stack.splice(index, 1);
    }
  };
}
