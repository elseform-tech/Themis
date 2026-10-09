import { useEffect, useState } from "react";
import { attachmentFile } from "../lib/tauri";

export function AttachmentPreview({ threadId, path }: { threadId: string; path: string }) {
  const name = path.split(/[\\/]/).pop() ?? path;
  const extension = name.split(".").pop()?.toLowerCase();
  const [url, setUrl] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let cancelled = false;
    setUrl(""); setError("");
    attachmentFile(threadId, path).then(value => { if (!cancelled) setUrl(value); })
      .catch(() => { if (!cancelled) setError("Preview unavailable"); });
    return () => { cancelled = true; };
  }, [threadId, path]);
  return <figure className="themis-attachment-preview" aria-label={name}>
    <div className="themis-attachment-media">
    {url && (/^(png|jpe?g|gif|webp|avif|bmp)$/.test(extension ?? "") ? <img src={url} alt={name} onError={() => setError("Preview unavailable")} />
      : extension === "pdf" ? (url.endsWith(".png") ? <img src={url} alt={`Preview ${name}`} /> : <iframe src={url} title={`Preview ${name}`} sandbox="" />)
      : /^(mp3|m4a|wav|ogg|flac|aac)$/.test(extension ?? "") ? <audio src={url} controls preload="metadata" aria-label={`Play ${name}`} onError={() => setError("Preview unavailable")} />
      : /^(mp4|webm|mov|m4v)$/.test(extension ?? "") ? <video src={url} controls preload="metadata" aria-label={`Play ${name}`} onError={() => setError("Preview unavailable")} /> : <svg width="32" height="40" viewBox="0 0 24 30" fill="none" stroke="currentColor" aria-hidden="true"><path d="M3 1h12l6 6v22H3zM15 1v6h6M7 14h10M7 19h10" /></svg>)}
    </div>
    <figcaption title={name}>{name}</figcaption>
    {error && <small role="status">{error}</small>}
  </figure>;
}
