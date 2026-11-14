// The still header picture: the board's shaded look (radial body, shadow overlay, highlight, the
// accessory with a drop shadow, a ground shadow) for one expression. It moves like the board
// unless reduced motion asks for stillness. Used by the channel header and as the 3D scene's
// fallback.
import { useId } from "react";
import { resolveLook, type Look } from "./cast";
import { EXPRESSION_LABEL, EXPRESSION_MOTION, faceFor, type Expression } from "./Expression";
import { reducedMotion } from "./thinking";
import "./Visions.css";

const INK = "#0B1220";

export function StillPicture({ look, expression, name = "", size = 130 }: { look: Look; expression: Expression; name?: string; size?: number }) {
  const uid = useId().replace(/:/g, "");
  const { shape, color, accessory, accessoryId } = resolveLook(look);
  const { l, r, y } = shape.eyes;
  const face = faceFor(expression, l, r, y);
  const still = reducedMotion();
  const label = `${name ? `${name}, ` : ""}${EXPRESSION_LABEL[expression]}`;
  return (
    <svg className="themis-still" width={size} height={size} viewBox="0 -9 48 57" role="img" aria-label={label} data-accessory={accessoryId} data-expression={expression} data-still={still ? "true" : "false"} focusable="false">
      <defs>
        <radialGradient id={`b${uid}`} cx="0.36" cy="0.26" r="0.86"><stop offset="0" stopColor={color.hi} /><stop offset="0.42" stopColor={color.mid} /><stop offset="1" stopColor={color.lo} /></radialGradient>
        <linearGradient id={`r${uid}`} x1="0" y1="0" x2="0" y2="1"><stop offset="0.5" stopColor="#0B1430" stopOpacity="0" /><stop offset="1" stopColor="#0B1430" stopOpacity="0.38" /></linearGradient>
        <filter id={`f${uid}`} x="-50%" y="-200%" width="200%" height="500%"><feGaussianBlur stdDeviation="1.4" /></filter>
      </defs>
      <ellipse cx="24.5" cy="45.6" rx="16" ry="2.2" fill="#08142D" fillOpacity="0.3" filter={`url(#f${uid})`} />
      <g className={`vc${still ? "" : ` ${EXPRESSION_MOTION[expression]}`}`} style={{ opacity: face.opacity }}>
        <path d={shape.body} fill={`url(#b${uid})`} />
        <path d={shape.body} fill={`url(#r${uid})`} />
        <ellipse cx={shape.hx} cy={shape.hy} rx="5.2" ry="2.1" transform={`rotate(-16 ${shape.hx} ${shape.hy})`} fill="#fff" fillOpacity=".6" />
        <path d={face.cheeks} fill="#FF8FB0" fillOpacity=".6" />
        <path className="e" d={face.eyesFill} fill={INK} />
        <path d={face.glint} fill="#fff" />
        <path className="e" d={face.eyesLine} fill="none" stroke={INK} strokeWidth="1.6" strokeLinecap="round" />
        <path d={face.brows} fill="none" stroke={INK} strokeWidth="1.2" strokeLinecap="round" />
        <path d={face.mouth} fill={face.mouthFill} stroke={INK} strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
        <path d={accessory.line} fill="none" stroke={accessory.lineColor} strokeWidth={accessory.lineWidth} strokeLinecap="round" strokeLinejoin="round" />
        <path d={accessory.solid} fill="#0B1430" fillOpacity=".22" transform="translate(.6 .7)" />
        <path d={accessory.solid} fill={accessory.fill} stroke={accessory.stroke} strokeWidth="1.1" strokeLinejoin="round" />
        <path d={accessory.inner} fill="none" stroke={accessory.ink} strokeWidth={accessory.innerWidth} strokeLinecap="round" strokeLinejoin="round" />
      </g>
    </svg>
  );
}
