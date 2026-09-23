import type { ReactNode } from "react";
import "./Badge.css";

export type BadgeTone =
  | "default"
  | "success"
  | "warning"
  | "danger"
  | "info"
  | "accent";

export interface BadgeProps {
  tone?: BadgeTone;
  children: ReactNode;
}

export function Badge({ tone = "default", children }: BadgeProps) {
  return (
    <span className={`themis-badge themis-badge--${tone}`}>{children}</span>
  );
}
