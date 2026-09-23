import { useEffect, useRef } from "react";
import "./Toasts.css";

export type ToastTone = "info" | "success" | "warning" | "danger";

export interface ToastItem {
  id: string;
  message: string;
  tone: ToastTone;
}

export interface ToastsProps {
  toasts: ToastItem[];
  onDismiss: (id: string) => void;
}

export function Toasts({ toasts, onDismiss }: ToastsProps) {
  if (toasts.length === 0) return null;
  return (
    <div aria-live="polite" className="themis-toasts">
      {toasts.map(toast => <Toast key={toast.id} toast={toast} onDismiss={onDismiss} />)}
    </div>
  );
}

function Toast({ toast, onDismiss }: { toast: ToastItem; onDismiss: ToastsProps["onDismiss"] }) {
  const dismiss = useRef(onDismiss);
  dismiss.current = onDismiss;
  useEffect(() => {
    const timer = window.setTimeout(() => dismiss.current(toast.id), 6000);
    return () => window.clearTimeout(timer);
  }, [toast.id]);
  return <div role="status" className={`themis-toast themis-toast--${toast.tone}`}>
    <span className="themis-toast-message">{toast.message}</span>
    <button type="button" aria-label="Dismiss notification" className="themis-toast-dismiss" onClick={() => onDismiss(toast.id)}>✕</button>
    <span className="themis-toast-timer" aria-hidden="true" />
  </div>;
}
