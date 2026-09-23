// Themis UI component library — barrel export.
// The screens agent imports components from here. Components are pure:
// they never touch ../lib/tauri; backend interaction arrives via callbacks.

export { Button } from "./primitives/Button";
export type {
  ButtonProps,
  ButtonSize,
  ButtonVariant,
} from "./primitives/Button";
export { Input } from "./primitives/Input";
export type { InputProps } from "./primitives/Input";
export { Dialog } from "./primitives/Dialog";
export type { DialogProps } from "./primitives/Dialog";
export { Dropdown } from "./primitives/Dropdown";
export type { DropdownItem, DropdownProps } from "./primitives/Dropdown";
export { Tabs } from "./primitives/Tabs";
export type { TabItem, TabsProps } from "./primitives/Tabs";
export { Tooltip } from "./primitives/Tooltip";
export type { TooltipProps } from "./primitives/Tooltip";
export { Badge } from "./primitives/Badge";
export type { BadgeProps, BadgeTone } from "./primitives/Badge";
export { StatusDot } from "./primitives/StatusDot";
export type { StatusDotProps, StatusKind } from "./primitives/StatusDot";
export { Toasts } from "./primitives/Toasts";
export type {
  ToastItem,
  ToastTone,
  ToastsProps,
} from "./primitives/Toasts";
export { EmptyState } from "./primitives/EmptyState";
export type { EmptyStateProps } from "./primitives/EmptyState";
export { BudgetMeter } from "./primitives/BudgetMeter";
export type { BudgetMeterProps } from "./primitives/BudgetMeter";

export { CodeBlock, tokenizeLine } from "./CodeBlock";
export type { CodeBlockProps } from "./CodeBlock";

export { DiffView } from "./diff/DiffView";
export type { DiffMode, DiffViewProps } from "./diff/DiffView";
export { FileTree } from "./diff/FileTree";
export type { FileTreeProps } from "./diff/FileTree";

export { ApprovalDialog } from "./approval/ApprovalDialog";
export type { ApprovalDialogProps } from "./approval/ApprovalDialog";
