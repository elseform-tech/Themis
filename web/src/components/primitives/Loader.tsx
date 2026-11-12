import "./Loader.css";

export interface LoaderProps {
  label: string;
  progress?: number;
}

export function Loader({ label, progress }: LoaderProps) {
  return (
    <div className="themis-loader" aria-busy="true">
      <span className="themis-wordmark">Themis</span>
      <div className={`themis-loader-bar${progress === undefined ? " is-indeterminate" : ""}`} role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress}>
        <div className="themis-loader-fill" style={progress === undefined ? undefined : { width: `${Math.max(0, Math.min(100, progress))}%` }} />
      </div>
      <p role="status">{label}</p>
    </div>
  );
}
