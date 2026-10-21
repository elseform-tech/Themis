import { useEffect, useState } from "react";
import { attachmentFile } from "../lib/tauri";

const IMAGE = /^(png|jpe?g|gif|webp|avif|bmp)$/;
const AUDIO = /^(mp3|m4a|wav|ogg|flac|aac)$/;
const VIDEO = /^(mp4|webm|mov|m4v)$/;

/** Sent or pending attachment: media as a rounded thumbnail, everything else as a compact file chip. */
export function AttachmentPreview({ threadId, path }: { threadId: string; path: string }) {
  const name = path.split(/[\\/]/).pop() ?? path;
  const extension = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  const [url, setUrl] = useState("");
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setUrl(""); setFailed(false);
    attachmentFile(threadId, path).then(value => { if (!cancelled) { if (value) setUrl(value); else setFailed(true); } })
      .catch(() => { if (!cancelled) setFailed(true); });
    return () => { cancelled = true; };
  }, [threadId, path]);
  const pdfImage = extension === "pdf" && url.endsWith(".png");
  const visual = !failed && (IMAGE.test(extension) || VIDEO.test(extension) || extension === "pdf");
  if (visual) {
    return <figure className="themis-attachment-preview themis-attachment-preview--media" aria-label={name} title={name}>
      <div className="themis-attachment-media">
        {url && (IMAGE.test(extension) || pdfImage ? <img src={url} alt={pdfImage ? `Preview ${name}` : name} onError={() => setFailed(true)} />
          : extension === "pdf" ? <iframe src={url} title={`Preview ${name}`} sandbox="" />
          : <video src={`${url}#t=0.1`} controls preload="metadata" playsInline aria-label={`Play ${name}`} onError={() => setFailed(true)} />)}
      </div>
      <figcaption>{name}</figcaption>
    </figure>;
  }
  return <figure className="themis-attachment-preview themis-attachment-preview--file" aria-label={name} title={name}>
    <span className="themis-attachment-ext" aria-hidden="true">{extension.slice(0, 4) || "file"}</span>
    <span className="themis-attachment-text">
      <figcaption>{name}</figcaption>
      {url && AUDIO.test(extension) ? <audio src={url} controls preload="metadata" aria-label={`Play ${name}`} onError={() => setFailed(true)} />
        : <small>{failed ? "Preview unavailable" : extension ? `${extension.toUpperCase()} file` : "File"}</small>}
    </span>
  </figure>;
}
