import { splitProps, Show, type ComponentProps, type ParentComponent } from "solid-js";
import clsx from "clsx";

// Common button style variants
export type DaisyButtonVariant =
  | "neutral"
  | "primary"
  | "secondary"
  | "accent"
  | "info"
  | "success"
  | "warning"
  | "error";

export type DaisyButtonSize = "xs" | "sm" | "md" | "lg" | "xl";

// Shared props for both button and link
type ButtonBaseProps = {
  /** Color variant — rendered as a translucent tint over the base */
  variant?: DaisyButtonVariant;
  /** Unified style: the same translucent pill as the default look */
  outline?: boolean;
  /** Unified style: the same translucent pill as the default look */
  dash?: boolean;
  /** Unified style: the same translucent pill as the default look */
  soft?: boolean;
  /** Unified style: the same translucent pill as the default look */
  ghost?: boolean;
  /** Looks like a link: no fill, underlined text */
  link?: boolean;

  /** No-op, kept for API compatibility */
  active?: boolean;
  /** Disabled state – adds the disabled attribute and dims the button */
  disabled?: boolean;
  /** Loading spinner state */
  loading?: boolean;

  /** Size – default is medium (no explicit class) */
  size?: DaisyButtonSize;
  /** Wide button (extra horizontal padding) */
  wide?: boolean;
  /** Full width button */
  block?: boolean;
  /** Square aspect ratio (1:1) */
  square?: boolean;
  /** Circle aspect ratio (1:1 with full rounded) */
  circle?: boolean;
};

export type ButtonProps = ButtonBaseProps &
  ComponentProps<"button"> & {
    /** Override type to only allow valid button types */
    type?: "button" | "submit" | "reset";
  };

// Unified button look: fully rounded (pill, or a circle when icon-only)
// with a semi-transparent fill; color variants become translucent tints.
const TINTS: Record<DaisyButtonVariant, string> = {
  neutral: "bg-base-content/10 text-base-content hover:bg-base-content/20",
  primary: "bg-primary/20 text-primary hover:bg-primary/30",
  secondary: "bg-secondary/20 text-secondary hover:bg-secondary/30",
  accent: "bg-accent/20 text-accent hover:bg-accent/30",
  info: "bg-info/20 text-info hover:bg-info/30",
  success: "bg-success/20 text-success hover:bg-success/30",
  warning: "bg-warning/20 text-warning hover:bg-warning/30",
  error: "bg-error/20 text-error hover:bg-error/30",
};

const PILL_SIZES: Record<DaisyButtonSize, string> = {
  xs: "h-6 gap-1 px-2.5 text-xs",
  sm: "h-8 gap-1.5 px-3 text-sm",
  md: "h-10 gap-2 px-4 text-sm",
  lg: "h-12 gap-2 px-5 text-base",
  xl: "h-14 gap-2 px-6 text-base",
};

const CIRCLE_SIZES: Record<DaisyButtonSize, string> = {
  xs: "size-6",
  sm: "size-8",
  md: "size-10",
  lg: "size-12",
  xl: "size-14",
};

export const Button: ParentComponent<ButtonProps> = (props) => {
  const [local, others] = splitProps(props, [
    "variant",
    "outline",
    "dash",
    "soft",
    "ghost",
    "link",
    "active",
    "disabled",
    "loading",
    "size",
    "wide",
    "block",
    "square",
    "circle",
    "class",
    "classList",
    "children",
    "type",
  ]);

  const className = () =>
    clsx(
      "inline-flex cursor-pointer select-none items-center justify-center whitespace-nowrap rounded-full font-medium transition-colors",
      "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary",
      "disabled:pointer-events-none disabled:opacity-40",
      TINTS[local.variant ?? "neutral"],
      local.link &&
        "bg-transparent underline underline-offset-2 hover:bg-transparent",
      local.wide && "px-8",
      local.block && "w-full",
      local.circle || local.square
        ? CIRCLE_SIZES[local.size ?? "md"]
        : PILL_SIZES[local.size ?? "md"],
      // User classes
      local.class,
    );

  return (
    <button
      type={local.type ?? "button"}
      disabled={local.disabled}
      aria-disabled={local.disabled || undefined}
      {...others}
      class={className()}
      classList={local.classList}
    >
      <Show when={local.loading}>
        <span class="loading loading-spinner"></span>
      </Show>
      {local.children}
    </button>
  );
};
