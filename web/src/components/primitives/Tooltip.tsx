import type { ReactNode } from "react";
import "./Tooltip.css";

export interface TooltipProps {
  content: string;
  children: ReactNode;
}

export function Tooltip({ content, children }: TooltipProps) {
  return (
    <span className="themis-tooltip" data-tooltip={content}>
      {children}
    </span>
  );
}
