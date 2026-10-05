import { splitProps, type ComponentProps, type ParentComponent } from "solid-js";
import { Transition } from "solid-transition-group";

export type TransitionScaleProps = ComponentProps<typeof Transition>;

/**
 * Appear/disappear via opacity + scale. The transform-origin is set on the
 * child element itself (e.g. `origin-top`), so the scaling "grows" from the
 * desired edge.
 */
export const TransitionScale: ParentComponent<TransitionScaleProps> = (
  props,
) => {
  const [, rest] = splitProps(props, [
    "children",
    "name",
    "enterClass",
    "enterActiveClass",
    "enterToClass",
    "exitClass",
    "exitActiveClass",
    "exitToClass",
  ]);
  return (
    <Transition
      enterClass="opacity-0 scale-95"
      enterActiveClass="transition duration-150 ease-out"
      enterToClass="opacity-100 scale-100"
      exitClass="opacity-100 scale-100"
      exitActiveClass="transition duration-100 ease-in"
      exitToClass="opacity-0 scale-95"
      {...rest}
    >
      {props.children}
    </Transition>
  );
};
