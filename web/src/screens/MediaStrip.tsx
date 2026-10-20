import { createContext, useContext, useEffect, useState } from "react";
import { Dialog } from "../components";
import { attachmentFile } from "../lib/tauri";
import type { AttachedMedia } from "../lib/types";
import "./MediaStrip.css";

/** Tiles shown before the rest fold into a "+N" tile. */
const SHOWN = 3;

/** Every media item of the chat's replies, so the viewer can step through all of them. */
export const MediaGallery = createContext<{ threadId: string; items: AttachedMedia[]; open: (path: string) => void } | null>(null);

function useMediaUrl(threadId: string, path: string) {
  const [url, setUrl] = useState("");
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let live = true;
    setUrl(""); setFailed(false);
    attachmentFile(threadId, path).then(value => { if (live) { if (value) setUrl(value); else setFailed(true); } }).catch(() => { if (live) setFailed(true); });
    return () => { live = false; };
  }, [threadId, path]);
  return { url, failed };
}

const label = (file: AttachedMedia) => file.caption || file.name;

/** The media an agent attached under its reply: a row of small thumbnails, or three and "+N". */
export function MediaStrip({ files }: { files: AttachedMedia[] }) {
  const gallery = useContext(MediaGallery);
  if (!gallery || files.length === 0) return null;
  const folded = files.length > SHOWN + 1;
  const shown = folded ? files.slice(0, SHOWN) : files;
  return (
    <ul className="themis-media-strip" aria-label="Attached media">
      {shown.map(file => <li key={file.path}><MediaTile file={file} /></li>)}
      {folded && (
        <li>
          <button type="button" className="themis-media-tile themis-media-more" aria-label={`Show ${files.length - SHOWN} more`} onClick={() => gallery.open(files[SHOWN].path)}>
            <span className="themis-media-frame"><span className="themis-media-count">+{files.length - SHOWN}</span></span>
            <span className="themis-media-caption">More</span>
          </button>
        </li>
      )}
    </ul>
  );
}

function MediaTile({ file }: { file: AttachedMedia }) {
  const gallery = useContext(MediaGallery)!;
  const { url, failed } = useMediaUrl(gallery.threadId, file.path);
  return (
    <button type="button" className="themis-media-tile" aria-label={`${file.kind === "video" ? "Play" : "Open"} ${label(file)}`} onClick={() => gallery.open(file.path)}>
      <span className="themis-media-frame">
        {failed ? <span className="themis-media-missing">Preview unavailable</span>
          : url && file.kind === "image" ? <img src={url} alt="" />
          : url && file.kind === "video" ? <><video src={url} preload="metadata" muted aria-hidden="true" tabIndex={-1} /><span className="themis-media-play" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z" /></svg></span></>
          : file.kind === "pdf" ? <span className="themis-media-missing">PDF</span> : null}
      </span>
      <span className="themis-media-caption">{label(file)}</span>
    </button>
  );
}

/** Full-size view of one item with previous/next through the chat's media. */
export function MediaViewer({ threadId, items, index, onIndex, onClose }: { threadId: string; items: AttachedMedia[]; index: number; onIndex: (index: number) => void; onClose: () => void }) {
  const file = items[index];
  const { url, failed } = useMediaUrl(threadId, file?.path ?? "");
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "ArrowLeft") onIndex((index + items.length - 1) % items.length);
      if (event.key === "ArrowRight") onIndex((index + 1) % items.length);
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [index, items.length, onIndex]);
  if (!file) return null;
  return (
    <Dialog open onClose={onClose} label={label(file)} className="themis-media-viewer">
      <div className="themis-media-viewer-head">
        <strong>{label(file)}</strong>
        {file.caption && <span className="themis-media-viewer-file">{file.name}</span>}
        <span className="themis-media-viewer-spacer" />
        {items.length > 1 && <span className="themis-media-viewer-count">{index + 1} of {items.length}</span>}
        <button type="button" className="themis-link-button" aria-label="Close viewer" onClick={onClose}>✕</button>
      </div>
      <div className="themis-media-viewer-body">
        {items.length > 1 && <button type="button" className="themis-media-step" aria-label="Previous" onClick={() => onIndex((index + items.length - 1) % items.length)}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="m15 6-6 6 6 6" /></svg></button>}
        <div className="themis-media-viewer-stage">
          {failed ? <p className="themis-workspace-note">Preview unavailable</p>
            : !url ? null
            : file.kind === "image" ? <img src={url} alt={label(file)} />
            : file.kind === "video" ? <video key={url} src={url} controls autoPlay aria-label={`Play ${label(file)}`} />
            : <iframe src={url} title={label(file)} sandbox="" />}
        </div>
        {items.length > 1 && <button type="button" className="themis-media-step" aria-label="Next" onClick={() => onIndex((index + 1) % items.length)}><svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true"><path d="m9 6 6 6-6 6" /></svg></button>}
      </div>
    </Dialog>
  );
}
