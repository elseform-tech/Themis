import "./Loader.css";

export interface LoaderProps {
  label: string;
  progress?: number;
  onWake?: () => void;
}

export function Loader({ label, progress, onWake }: LoaderProps) {
  return (
    <div className="themis-loader" aria-busy="true">
      <svg className="themis-loader-logo" viewBox="0 0 32 32" aria-hidden="true">
        <circle cx="16" cy="16" r="13" fill="none" stroke="currentColor" strokeWidth="2.5" />
        <circle cx="16" cy="16" r="4.5" fill="currentColor" />
      </svg>
      <span className="themis-wordmark">Themis</span>
      {progress !== undefined && (
        <div className="themis-loader-bar" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress}>
          <div className="themis-loader-fill" style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} />
        </div>
      )}
      <p role="status">{label}</p>
      {onWake && <button type="button" className="themis-loader-wake" onClick={onWake}>Wake</button>}
    </div>
  );
}
