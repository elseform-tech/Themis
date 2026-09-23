import "./EmptyState.css";

export interface EmptyStateProps {
  title: string;
  hint?: string;
}

export function EmptyState({ title, hint }: EmptyStateProps) {
  return (
    <div className="themis-empty">
      <p className="themis-empty-title">{title}</p>
      {hint !== undefined && hint !== "" && (
        <p className="themis-empty-hint">{hint}</p>
      )}
    </div>
  );
}
