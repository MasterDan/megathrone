import { splitProps, type ComponentProps, type ParentComponent } from "solid-js";
import { Transition } from "solid-transition-group";

export type TransitionSheetProps = ComponentProps<typeof Transition>;

/**
 * Mobile bottom-sheet variant of the modal transition: the panel slides
 * up from the screen bottom edge (no scale — a sheet is anchored to the
 * edge it comes from). Used by Modal below the Tailwind `sm` breakpoint.
 */
export const TransitionSheet: ParentComponent<TransitionSheetProps> = (
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
      enterClass="translate-y-full"
      enterActiveClass="transition duration-200 ease-out"
      enterToClass=""
      exitClass=""
      exitActiveClass="transition duration-150 ease-in"
      exitToClass="translate-y-full"
      {...rest}
    >
      {props.children}
    </Transition>
  );
};
