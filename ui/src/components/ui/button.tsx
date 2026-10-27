import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        outline: "border border-input bg-background hover:bg-accent hover:text-accent-foreground",
        ghost: "hover:bg-accent hover:text-accent-foreground",
        destructive: "bg-destructive text-destructive-foreground hover:bg-destructive/90",
        // the one action a screen exists for (sign in, accept an invite, turn on
        // two-factor): the folk-red fill with white text, pressing to --red-700.
        // 6.69:1 for the label on --red-folk. one red action per view (DESIGN.md)
        brand: "bg-brand-folk text-white hover:bg-brand-press",
      },
      size: {
        // a coarse pointer (a finger) gets a 44px floor (#2040); desktop density is unchanged
        default: "h-9 px-4 py-2 pointer-coarse:h-11",
        sm: "h-8 rounded-md px-3 text-xs pointer-coarse:h-11",
        lg: "h-10 rounded-md px-8 pointer-coarse:h-11",
        icon: "h-9 w-9 pointer-coarse:h-11 pointer-coarse:w-11",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {}

// `type` defaults to "button": a native button inside a <form> submits it,
// which is never what a Cancel or a Delete placed in a form-backed sheet means.
// submit buttons say so explicitly
const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, type = "button", ...props }, ref) => (
    <button
      ref={ref}
      type={type}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  ),
);
Button.displayName = "Button";

export { Button, buttonVariants };
