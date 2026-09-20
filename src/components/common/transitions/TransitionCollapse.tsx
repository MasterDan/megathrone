import type { ComponentProps, ParentComponent } from "solid-js";
import { splitProps } from "solid-js";
import { Transition } from "solid-transition-group";

export type TransitionCollapseProps = ComponentProps<typeof Transition>;

/**
 * Vertical reveal/collapse via `grid-template-rows: 0fr → 1fr` + opacity:
 * the block animates to its natural height — no max-height guessing, works
 * for arbitrarily tall content. The transition classes must never touch
 * the content's own display (a flex row would turn into a grid for the
 * duration of the animation), so the wrapper carries its own grid
 * container; the `min-h-0` item defeats the grid automatic minimum that
 * would otherwise keep the 0fr track content-sized, and the explicit
 * `minmax(0, 1fr)` column (`grid-cols-1`) stops an implicit auto track
 * from blowing the container out to the content's max-content width
 * (nowrap code lines etc.). The content's spacing must live inside the
 * animated block — a parent's gap/margin would snap when the node
 * unmounts.
 */
export const TransitionCollapse: ParentComponent<TransitionCollapseProps> = (
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
      enterClass="grid-rows-[0fr] opacity-0"
      enterActiveClass="overflow-hidden transition-all duration-300 ease-in-out"
      enterToClass="grid-rows-[1fr] opacity-100"
      exitClass="grid-rows-[1fr] opacity-100"
      exitActiveClass="overflow-hidden transition-all duration-200 ease-in-out"
      exitToClass="grid-rows-[0fr] opacity-0"
      {...rest}
    >
      <div class="grid grid-cols-1">
        <div class="min-h-0">{props.children}</div>
      </div>
    </Transition>
  );
};
