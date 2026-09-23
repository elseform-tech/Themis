import type { ButtonHTMLAttributes, ReactNode } from "react";
import "./Button.css";

export type ButtonVariant = "primary" | "ghost" | "danger";
export type ButtonSize = "small" | "medium";

export interface ButtonProps
  extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  children: ReactNode;
}

export function Button({
  variant = "primary",
  size = "medium",
  type = "button",
  className,
  children,
  ...rest
}: ButtonProps) {
  const classes = [
    "themis-button",
    `themis-button--${variant}`,
    `themis-button--${size}`,
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type={type} className={classes} {...rest}>
      {children}
    </button>
  );
}
