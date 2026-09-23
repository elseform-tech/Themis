import { Children, isValidElement, useEffect, useId, useMemo, useState } from "react";
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
  const viewBox = /viewBox="[^"]*?([\d.]+)\s+([\d.]+)"/.exec(svg);
  const height = viewBox ? Math.min(560, Math.max(120, Number(viewBox[2]) + 24)) : 300;
  return <div className="themis-diagram">
    {svg ? <iframe title="Mermaid diagram" sandbox="" style={{ height }} srcDoc={`<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:"><style>body{margin:0;padding:12px;box-sizing:border-box}svg{max-width:100%;height:auto;display:block;margin:auto}</style>${svg}`} /> : !error && <p>Rendering diagram…</p>}
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
