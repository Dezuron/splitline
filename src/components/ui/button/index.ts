import { cva, type VariantProps } from "class-variance-authority";

/**
 * Buttons share one geometry and take every colour from the `--btn-*` tokens,
 * so a theme only changes colours. No gradients, no per-theme overrides.
 */
export const buttonVariants = cva(
  "ui-button inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-[var(--btn-radius)] text-[length:var(--fs-label)] font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-45",
  {
    variants: {
      variant: {
        default:
          "border border-[var(--btn-primary-border)] bg-[var(--btn-primary-bg)] text-[var(--btn-primary-fg)] hover:bg-[var(--btn-primary-bg-hover)]",
        secondary:
          "border border-[var(--btn-border)] bg-[var(--btn-bg)] text-[var(--btn-fg)] hover:bg-[var(--btn-bg-hover)]",
        outline:
          "border border-[var(--btn-border)] bg-transparent text-[var(--text-2)] hover:bg-[var(--btn-bg-hover)] hover:text-[var(--text)]",
        ghost:
          "border border-transparent text-[var(--text-2)] hover:bg-[var(--hover-bg)] hover:text-[var(--text)]",
        destructive:
          "border border-[var(--btn-danger-border)] bg-[var(--btn-danger-bg)] text-[var(--btn-danger-fg)] hover:bg-[var(--btn-danger-bg-hover)]",
      },
      size: {
        default: "h-[var(--control-h)] px-3.5",
        sm: "h-[var(--control-h-sm)] px-2.5",
        lg: "h-[44px] px-5",
        icon: "h-[var(--control-h)] w-[var(--control-h)]",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export type ButtonVariants = VariantProps<typeof buttonVariants>;
