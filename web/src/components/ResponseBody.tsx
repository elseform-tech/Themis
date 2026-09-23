import { Children, isValidElement, useEffect, useId, useMemo, useRef, useState } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { CodeBlock } from "./CodeBlock";
import "./ResponseBody.css";

function CodePanel({ code, language, streaming }: { code: string; language: string; streaming: boolean }) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  return <div className="themis-code-panel"><div className="themis-code-toolbar"><span>{language || "text"}</span><button onClick={async () => { try { await navigator.clipboard.writeText(code); setCopied(true); setCopyError(false); } catch { setCopyError(true); } }}>{copyError ? "Select to copy" : copied ? "Copied" : "Copy"}</button></div>
    {language === "mermaid" && !streaming ? <Diagram code={code} /> : <CodeBlock code={code} language={language} />}
  </div>;
}

function Diagram({ code }: { code: string }) {
  const id = `diagram-${useId().replace(/[^a-zA-Z0-9]/g, "")}`;
  const [svg, setSvg] = useState("");
  const [error, setError] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const dialog = useRef<HTMLDialogElement>(null);
  const pan = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const [theme, setTheme] = useState(document.documentElement.dataset.theme);
  useEffect(() => {
    const observer = new MutationObserver(() => setTheme(document.documentElement.dataset.theme));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let cancelled = false;
    setSvg(""); setError(false);
    // Diagrams are local visuals. Do not fetch model-supplied image resources.
    if (code.length > 50000 || /^\s*---|%%\s*\{/m.test(code) || /@\{[^}]*\bimg\s*:|<\s*(img|image|iframe|script)\b/i.test(code)) { setError(true); return; }
    void import("mermaid").then(async ({ default: mermaid }) => {
      if (cancelled) return;
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: theme === "dark" ? "dark" : "default", suppressErrorRendering: true, maxTextSize: 50000, maxEdges: 500, secure: ["secure", "securityLevel", "startOnLoad", "maxTextSize", "maxEdges", "suppressErrorRendering", "dompurifyConfig"] });
      const result = await mermaid.render(id, code);
      if (!cancelled) setSvg(result.svg);
    }).catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; };
  }, [code, theme, id]);
  useEffect(() => {
    if (expanded) dialog.current?.showModal(); else dialog.current?.close();
  }, [expanded]);
  const viewBox = /viewBox="\s*[-\d.]+\s+[-\d.]+\s+([\d.]+)\s+([\d.]+)"/.exec(svg);
  const width = Math.max(320, Number(viewBox?.[1] ?? 800));
  const height = Math.max(160, Number(viewBox?.[2] ?? 300));
  const srcDoc = `<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:"><style>body{margin:0;padding:12px;box-sizing:border-box}svg{width:100%!important;max-width:none!important;height:auto!important;display:block}</style>${svg}`;
  const frame = (scale: number) => <iframe title="Mermaid diagram" sandbox="" draggable={false} style={{ width: width * scale + 24, height: height * scale + 24 }} srcDoc={srcDoc} />;
  return <div className="themis-diagram">
    {svg ? <><div className="themis-diagram-preview">{frame(1)}<button className="themis-diagram-expand" aria-label="Enlarge diagram" title="Enlarge diagram" onClick={() => { setZoom(1); setOffset({ x: 0, y: 0 }); setExpanded(true); }}><svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/><path d="M3 3l6 6m12-6-6 6M3 21l6-6m12 6-6-6"/></svg></button></div>
      {expanded && <dialog ref={dialog} className="themis-diagram-dialog" onClose={() => setExpanded(false)} aria-label="Enlarged Mermaid diagram">
        <div className="themis-diagram-toolbar"><strong>Diagram</strong><button onClick={() => setZoom(Math.max(.5, +(zoom - .25).toFixed(2)))} aria-label="Zoom out">−</button><span>{Math.round(zoom * 100)}%</span><button onClick={() => setZoom(Math.min(3, +(zoom + .25).toFixed(2)))} aria-label="Zoom in">+</button><button onClick={() => { setZoom(1); setOffset({ x: 0, y: 0 }); }}>100%</button><button onClick={() => { const canvas = dialog.current?.querySelector(".themis-diagram-canvas"); if (canvas) { setZoom(Math.min(3, Math.max(.5, Math.min((canvas.clientWidth - 64) / (width + 24), (canvas.clientHeight - 64) / (height + 24))))); setOffset({ x: 0, y: 0 }); } }}>Fit</button><button onClick={() => setExpanded(false)} aria-label="Close diagram">Close</button></div>
        <div className="themis-diagram-canvas" onPointerDown={(event) => { if (event.button !== 0) return; pan.current = { x: event.clientX, y: event.clientY, left: offset.x, top: offset.y }; event.currentTarget.setPointerCapture(event.pointerId); }} onPointerMove={(event) => { if (!pan.current) return; setOffset({ x: pan.current.left + event.clientX - pan.current.x, y: pan.current.top + event.clientY - pan.current.y }); }} onPointerUp={() => { pan.current = null; }} onPointerCancel={() => { pan.current = null; }}><div className="themis-diagram-stage" style={{ width: width * zoom + 24, height: height * zoom + 24, transform: `translateX(-50%) translate(${offset.x}px, ${offset.y}px)` }}>{frame(zoom)}</div></div>
      </dialog>}</> : !error && <p>Rendering diagram…</p>}
    <details open={error || undefined}><summary>{error ? "Diagram unavailable · View source" : "View source"}</summary><CodeBlock code={code} language="mermaid" /></details>
  </div>;
}

export function ResponseBody({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const components = useMemo<Components>(() => ({
    pre({ children }) {
      const child = Children.toArray(children)[0];
      if (!isValidElement<{ className?: string; children?: string }>(child)) return <pre>{children}</pre>;
      const language = /language-([^\s]+)/.exec(child.props.className ?? "")?.[1] ?? "";
      return <CodePanel code={String(child.props.children ?? "").replace(/\n$/, "")} language={language.toLowerCase()} streaming={streaming} />;
    },
    a({ children, href }) { return <a href={href} target="_blank" rel="noreferrer noopener">{children}</a>; },
    img({ alt, src }) { return <a href={typeof src === "string" ? src : undefined} target="_blank" rel="noreferrer noopener">{alt || "Image"}</a>; },
    table({ children }) { return <div className="themis-response-table"><table>{children}</table></div>; },
  }), [streaming]);
  return <div className="themis-response"><Markdown remarkPlugins={[remarkGfm]} skipHtml components={components}>{text}</Markdown></div>;
}
