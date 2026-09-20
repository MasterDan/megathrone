import { createEffect, createSignal, type Accessor, type Setter } from "solid-js";

/**
 * Two-way binding of a controlled prop and an internal signal.
 *
 * `[value, setValue] = createModel([props.value, onChange])` — after the call
 * the component works **only** with `value`/`setValue`; the prop is not read
 * anymore.
 *
 * Both arguments are optional:
 * neither given      — uncontrolled mode
 * only external      — uncontrolled mode with the initial value taken from it
 * only onChange      — uncontrolled mode with a "subscription" to changes
 */
export function createModel<T>(
  controlled: [
    external: Accessor<T> | undefined,
    onChange: ((value: T) => void) | undefined,
  ],
): [Accessor<T>, Setter<T>] {
  const [external, onChange] = controlled;
  const [internal, setInternal] = createSignal<T>(external?.() as T);

  if (!!external) {
    createEffect(() => {
      setInternal(() => external());
    });
  }

  if (!!onChange) {
    createEffect(() => {
      onChange(internal());
    });
  }

  return [internal, setInternal];
}
