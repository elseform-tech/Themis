import { Children, createContext, isValidElement, useContext, useEffect, useId, useMemo, useRef, useState } from "react";
import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { CodeBlock } from "./CodeBlock";
import { DIAGRAM_DARK, DIAGRAM_LIGHT } from "../theme/diagram";
import "./ResponseBody.css";

function CodePanel({ code, language, streaming }: { code: string; language: string; streaming: boolean }) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  return <div className={`themis-code-panel${language === "mermaid" ? " themis-code-panel--diagram" : ""}`}><div className="themis-code-toolbar"><span>{language || "text"}</span><button onClick={async () => { try { await navigator.clipboard.writeText(code); setCopied(true); setCopyError(false); } catch { setCopyError(true); } }}>{copyError ? "Select to copy" : copied ? "Copied" : "Copy"}</button></div>
    {language === "mermaid" ? (streaming ? <DiagramLoading /> : <Diagram code={code} />) : <CodeBlock code={code} language={language} />}
  </div>;
}

/** Every chart gets the same frame in the reply, and its graph fits inside. Enlarge shows the detail. */
const DIAGRAM_FRAME_HEIGHT = 280;
/** The frame's width when the chart can't measure it: the chat's column. */
const DIAGRAM_FRAME_WIDTH = 620;
/** A small chart grows to use the frame up to this, where its labels are comfortably large. */
const DIAGRAM_MOST_GROWTH = 1.5;

/** How large a chart this size can be drawn in a frame this size. */
export function fitScale(width: number, height: number, frameWidth: number, frameHeight: number): number {
  return Math.min(frameWidth / width, frameHeight / height);
}

/** The flowchart turned the other way round; null for any other kind of chart, which has one way. */
export function flipDirection(code: string): string | null {
  const opening = /^(\s*(?:flowchart|graph)\s+)(TD|TB|BT|LR|RL)\b/.exec(code);
  if (!opening) return null;
  return `${opening[1]}${opening[2] === "LR" || opening[2] === "RL" ? "TD" : "LR"}${code.slice(opening[0].length)}`;
}

function drawnSize(svg: string): [number, number] | null {
  const box = /viewBox="\s*[-\d.]+\s+[-\d.]+\s+([\d.]+)\s+([\d.]+)"/.exec(svg);
  return box ? [Number(box[1]), Number(box[2])] : null;
}

/** Of a flowchart's two directions, the one that comes out largest in the frame. The chart as
 *  written stands if the other can't be drawn: this only ever improves on it. */
async function roomiest(svg: string, code: string, frameWidth: number, draw: (code: string) => Promise<string>): Promise<string> {
  const flipped = flipDirection(code);
  const size = drawnSize(svg);
  if (!flipped || !size) return svg;
  try {
    const other = await draw(flipped);
    const otherSize = drawnSize(other);
    if (!otherSize) return svg;
    const scale = (found: [number, number]) => Math.min(DIAGRAM_MOST_GROWTH, fitScale(found[0], found[1], frameWidth - 16, DIAGRAM_FRAME_HEIGHT - 16));
    return scale(otherSize) > scale(size) ? other : svg;
  } catch {
    return svg;
  }
}
/** Enlarged, a huge chart can go this small to fit the window. */
const DIAGRAM_SMALLEST_ZOOM = 0.25;

/** Holds the chart's place, at its size, while it's written and drawn. */
function DiagramLoading() {
  return <div className="themis-diagram-loading" role="status" aria-label="Drawing chart" style={{ height: DIAGRAM_FRAME_HEIGHT }} />;
}

function Diagram({ code }: { code: string }) {
  const id = `diagram-${useId().replace(/[^a-zA-Z0-9]/g, "")}`;
  const [svg, setSvg] = useState("");
  const [error, setError] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const dialog = useRef<HTMLDialogElement>(null);
  const column = useRef<HTMLDivElement>(null);
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
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: "base", themeVariables: theme === "dark" ? DIAGRAM_DARK : DIAGRAM_LIGHT, flowchart: { nodeSpacing: 28, rankSpacing: 32, padding: 8, wrappingWidth: 140 }, suppressErrorRendering: true, maxTextSize: 50000, maxEdges: 500, secure: ["secure", "securityLevel", "startOnLoad", "maxTextSize", "maxEdges", "suppressErrorRendering", "dompurifyConfig"] });
      const drawn = await mermaid.render(id, code);
      if (cancelled) return;
      setSvg(await roomiest(drawn.svg, code, column.current?.clientWidth || DIAGRAM_FRAME_WIDTH, async flipped => (await mermaid.render(`${id}-flipped`, flipped)).svg));
    }).catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; };
  }, [code, theme, id]);
  const viewBoxFound = /viewBox="\s*[-\d.]+\s+[-\d.]+\s+([\d.]+)\s+([\d.]+)"/.exec(svg);
  const ownWidth = Number(viewBoxFound?.[1] ?? 800);
  const ownHeight = Number(viewBoxFound?.[2] ?? 300);
  function fitToWindow() {
    setOffset({ x: 0, y: 0 });
    const canvas = dialog.current?.querySelector(".themis-diagram-canvas");
    if (!canvas?.clientWidth || !canvas.clientHeight) return;
    const fits = Math.min((canvas.clientWidth - 64) / (ownWidth + 24), (canvas.clientHeight - 64) / (ownHeight + 24));
    setZoom(+Math.min(1, Math.max(DIAGRAM_SMALLEST_ZOOM, fits)).toFixed(2));
  }
  useEffect(() => {
    if (expanded) { dialog.current?.showModal(); fitToWindow(); } else dialog.current?.close();
  }, [expanded]); // eslint-disable-line react-hooks/exhaustive-deps
  // Filled by the graph whole and centred; the preview also holds a small chart at its own size.
  const page = (limit: string) => `<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:"><style>:root{color-scheme:${theme === "dark" ? "dark" : "light"}}html,body{height:100%;background:transparent}body{margin:0;padding:8px;box-sizing:border-box;display:flex;align-items:center;justify-content:center;overflow:hidden}svg{display:block;width:100%!important;height:100%!important;${limit.replace(/;/g, "!important;")}!important;object-fit:contain}</style>${svg}`;
  const frame = (scale: number) => <iframe title="Mermaid diagram" sandbox="" draggable={false} style={{ width: ownWidth * scale + 24, height: ownHeight * scale + 24 }} srcDoc={page("max-width:none")} />;
  return <div className="themis-diagram">
    {svg ? <><div className="themis-diagram-preview" ref={column}><iframe title="Mermaid diagram" sandbox="" draggable={false} style={{ width: "100%", height: DIAGRAM_FRAME_HEIGHT }} srcDoc={page(`max-width:${Math.round(ownWidth * DIAGRAM_MOST_GROWTH)}px;max-height:${Math.round(ownHeight * DIAGRAM_MOST_GROWTH)}px`)} /><button className="themis-diagram-expand" aria-label="Enlarge diagram" title="Enlarge diagram" onClick={() => { setZoom(1); setOffset({ x: 0, y: 0 }); setExpanded(true); }}><svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5"/><path d="M3 3l6 6m12-6-6 6M3 21l6-6m12 6-6-6"/></svg></button></div>
      {expanded && <dialog ref={dialog} className="themis-diagram-dialog" onClose={() => setExpanded(false)} aria-label="Enlarged Mermaid diagram">
        <div className="themis-diagram-toolbar"><strong>Diagram</strong><button onClick={() => setZoom(Math.max(DIAGRAM_SMALLEST_ZOOM, +(zoom - .25).toFixed(2)))} aria-label="Zoom out">−</button><span>{Math.round(zoom * 100)}%</span><button onClick={() => setZoom(Math.min(3, +(zoom + .25).toFixed(2)))} aria-label="Zoom in">+</button><button onClick={() => { setZoom(1); setOffset({ x: 0, y: 0 }); }}>100%</button><button onClick={fitToWindow}>Fit</button><button onClick={() => setExpanded(false)} aria-label="Close diagram">Close</button></div>
        <div className="themis-diagram-canvas" onPointerDown={(event) => { if (event.button !== 0) return; pan.current = { x: event.clientX, y: event.clientY, left: offset.x, top: offset.y }; event.currentTarget.setPointerCapture(event.pointerId); }} onPointerMove={(event) => { if (!pan.current) return; setOffset({ x: pan.current.left + event.clientX - pan.current.x, y: pan.current.top + event.clientY - pan.current.y }); }} onPointerUp={() => { pan.current = null; }} onPointerCancel={() => { pan.current = null; }}><div className="themis-diagram-stage" style={{ width: ownWidth * zoom + 24, height: ownHeight * zoom + 24, transform: `translateX(-50%) translate(${offset.x}px, ${offset.y}px)` }}>{frame(zoom)}</div></div>
      </dialog>}</> : !error && <DiagramLoading />}
    <details open={error || undefined}><summary>{error ? "Diagram unavailable · View source" : "View source"}</summary><CodeBlock code={code} language="mermaid" /></details>
  </div>;
}

/** Opens a reply's web link in the chat's built-in browser; without it links open in the system browser. */
export const ResponseLinks = createContext<((url: string) => void) | null>(null);

/** Opens a file a reply names, in Workspace. */
export const ResponseFiles = createContext<((path: string) => void) | null>(null);

const FILE_EXTENSION = /\.(?:[cm]?[jt]sx?|json[cl]?|md|txt|html?|css|scss|py|rs|go|rb|java|kt|swift|c|h|cpp|hpp|cs|php|sh|zsh|toml|ya?ml|xml|csv|sql|lock|env|ini|cfg|svg|png|jpe?g|gif|webp|pdf|ipynb)$/i;

/** The file a piece of inline code cites by line, without its `:line` ending. A file named in
 *  passing isn't evidence, so it stays plain code, like words, versions and `object.method`. */
export function fileNamedIn(code: string): string | null {
  const text = code.trim();
  if (!/^[\w@./~-]+:\d+(?:[-:]\d+)?$/.test(text) || /^\d/.test(text) || text.includes("//")) return null;
  const path = text.replace(/:\d+(?:[-:]\d+)?$/, "");
  if (path.endsWith("/") || path.startsWith("-")) return null;
  return FILE_EXTENSION.test(path) ? path : null;
}

const REMARK_PLUGINS = [remarkGfm];

export function ResponseBody({ text, streaming = false, artifacts }: { text: string; streaming?: boolean; /** Files this reply attached: naming one links to it. */ artifacts?: string[] }) {
  const artifactKey = artifacts?.join("\n");
  const openLink = useContext(ResponseLinks);
  const openFile = useContext(ResponseFiles);
  // The chat builds this list on every update; an equal one must not count as a change.
  const attached = useMemo(() => artifactKey?.split("\n"), [artifactKey]);
  const components = useMemo<Components>(() => ({
    pre({ children }) {
      const child = Children.toArray(children)[0];
      if (!isValidElement<{ className?: string; children?: string }>(child)) return <pre>{children}</pre>;
      const language = /language-([^\s]+)/.exec(child.props.className ?? "")?.[1] ?? "";
      return <CodePanel code={String(child.props.children ?? "").replace(/\n$/, "")} language={language.toLowerCase()} streaming={streaming} />;
    },
    code({ children, className }) {
      const text = String(children ?? "");
      if (className || text.includes("\n")) return <code className={className}>{children}</code>;
      // A web address, a cited line or an attached file goes somewhere.
      if (openLink && /^https?:\/\/\S+$/i.test(text)) {
        return <a href={text} target="_blank" rel="noreferrer noopener" onClick={event => { if (event.metaKey || event.ctrlKey) return; event.preventDefault(); openLink(text); }}><code>{text}</code></a>;
      }
      const artifact = attached?.find(path => path === text || path.split("/").pop() === text);
      const file = openFile ? artifact ?? fileNamedIn(text) : null;
      if (file && openFile) return <button type="button" className="themis-code-link" title={`Open ${file}`} onClick={() => openFile(file)}><code>{text}</code></button>;
      return <code>{children}</code>;
    },
    a({ children, href }) {
      // Cmd/Ctrl-click keeps the system browser.
      const inApp = openLink && href && /^https?:\/\//i.test(href);
      return <a href={href} target="_blank" rel="noreferrer noopener" onClick={inApp ? event => { if (event.metaKey || event.ctrlKey) return; event.preventDefault(); openLink(href); } : undefined}>{children}</a>;
    },
    img({ alt, src }) { return <a href={typeof src === "string" ? src : undefined} target="_blank" rel="noreferrer noopener">{alt || "Image"}</a>; },
    table({ children }) { return <div className="themis-response-table"><table>{children}</table></div>; },
  }), [streaming, openLink, openFile, attached]);
  // Keeping the element keeps React from parsing a finished reply again each time the chat updates.
  return useMemo(() => <div className="themis-response"><Markdown remarkPlugins={REMARK_PLUGINS} skipHtml components={components}>{text}</Markdown></div>, [text, components]);
}
