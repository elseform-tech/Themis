import type { InputHTMLAttributes } from "react";
import "./Input.css";

export interface InputProps
  extends Omit<InputHTMLAttributes<HTMLInputElement>, "id"> {
  id: string;
  label?: string;
}

export function Input({ id, label, className, ...rest }: InputProps) {
  return (
    <div className="themis-input-wrap">
      {label !== undefined && label !== "" && (
        <label htmlFor={id} className="themis-input-label">
          {label}
        </label>
      )}
      <input
        id={id}
        className={["themis-input", className].filter(Boolean).join(" ")}
        {...rest}
      />
    </div>
  );
}
