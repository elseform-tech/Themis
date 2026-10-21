import type { ReactNode } from "react";
import "./Tooltip.css";

export interface TooltipProps {
  content: string;
  children: ReactNode;
  /** Below the control, for controls along the top edge of a panel. */
  below?: boolean;
}

export function Tooltip({ content, children, below = false }: TooltipProps) {
  return (
    <span className={`themis-tooltip${below ? " themis-tooltip--below" : ""}`} data-tooltip={content}>
      {children}
    </span>
  );
}
