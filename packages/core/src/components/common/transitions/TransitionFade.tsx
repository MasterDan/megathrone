import { splitProps, type ComponentProps, type ParentComponent } from "solid-js";
import { Transition } from "solid-transition-group";

export type TransitionFadeProps = ComponentProps<typeof Transition>;

export const TransitionFade: ParentComponent<TransitionFadeProps> = (props) => {
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
      enterClass="opacity-0"
      enterActiveClass="transition-opacity duration-500"
      enterToClass="opacity-100"
      exitClass="opacity-100"
      exitActiveClass="transition-opacity duration-500"
      exitToClass="opacity-0"
      {...rest}
    >
      {props.children}
    </Transition>
  );
};
