import type { InputHTMLAttributes } from "react";
import "./Switch.css";

/** A native checkbox drawn as an on/off switch; it keeps checkbox semantics and keyboard behavior. */
export function Switch(props: Omit<InputHTMLAttributes<HTMLInputElement>, "type">) {
  return <input {...props} type="checkbox" className={props.className ? `themis-switch ${props.className}` : "themis-switch"} />;
}
