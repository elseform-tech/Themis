// The still picture (board Cloud3D): the one outline shaded (radial body, shadow overlay, rim light,
// highlight), eyes and brows only, the accessory and a ground shadow, for one expression. It moves like the board
// unless reduced motion asks for stillness. Used by the channel's welcome and as the 3D scene's
// fallback.
import { useId } from "react";
import { EYES, HIGHLIGHT, OUTLINE, resolveLook, type Look } from "./cast";
import { EXPRESSION_LABEL, EXPRESSION_MOTION, faceFor, type Expression } from "./Expression";
import { reducedMotion } from "./thinking";
import "./Visions.css";

const INK = "#0B1220";

export function StillPicture({ look, expression, name = "", size = 130 }: { look: Look; expression: Expression; name?: string; size?: number }) {
  const uid = useId().replace(/:/g, "");
  const { color, accessory, accessoryId } = resolveLook(look);
  const { l, r, y } = EYES;
  const face = faceFor(expression, l, r, y);
  const still = reducedMotion();
  const label = `${name ? `${name}, ` : ""}${EXPRESSION_LABEL[expression]}`;
  return (
    <svg className="themis-still" width={size} height={size} viewBox="0 0 48 48" role="img" aria-label={label} data-accessory={accessoryId} data-expression={expression} data-still={still ? "true" : "false"} focusable="false">
      <defs>
        <radialGradient id={`b${uid}`} cx="0.34" cy="0.22" r="0.95"><stop offset="0" stopColor={color.hi} /><stop offset="0.45" stopColor={color.mid} /><stop offset="1" stopColor={color.lo} /></radialGradient>
        <linearGradient id={`r${uid}`} x1="0" y1="0" x2="0" y2="1"><stop offset="0.5" stopColor="#0B1430" stopOpacity="0" /><stop offset="1" stopColor="#0B1430" stopOpacity="0.38" /></linearGradient>
        <linearGradient id={`l${uid}`} x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#fff" stopOpacity=".7" /><stop offset=".45" stopColor="#fff" stopOpacity="0" /></linearGradient>
        <filter id={`f${uid}`} x="-50%" y="-200%" width="200%" height="500%"><feGaussianBlur stdDeviation="1.2" /></filter>
      </defs>
      <ellipse cx="23.5" cy="43.6" rx="17" ry="2" fill="#08142D" fillOpacity="0.3" filter={`url(#f${uid})`} />
      <g className={`vc${still ? "" : ` ${EXPRESSION_MOTION[expression]}`}`} style={{ opacity: face.opacity }}>
        <path d={OUTLINE} fill={`url(#b${uid})`} />
        <path d={OUTLINE} fill={`url(#r${uid})`} />
        <path d={OUTLINE} fill="none" stroke={`url(#l${uid})`} strokeWidth=".8" />
        <ellipse cx={HIGHLIGHT.x} cy={HIGHLIGHT.y} rx="4.6" ry="1.7" transform={`rotate(-12 ${HIGHLIGHT.x} ${HIGHLIGHT.y})`} fill="#fff" fillOpacity=".55" />
        <path className="e" d={face.eyesFill} fill={INK} />
        <path d={face.glint} fill="#fff" />
        <path className="e" d={face.eyesLine} fill="none" stroke={INK} strokeWidth="1.6" strokeLinecap="round" />
        <path d={face.brows} fill="none" stroke={INK} strokeWidth="1.2" strokeLinecap="round" />
        <path d={accessory.line} fill="none" stroke={accessory.lineColor} strokeWidth={accessory.lineWidth} strokeLinecap="round" strokeLinejoin="round" />
        <path d={accessory.solid} fill="#0B1430" fillOpacity=".22" transform="translate(.6 .7)" />
        <path d={accessory.solid} fill={accessory.fill} stroke={accessory.stroke} strokeWidth=".9" strokeLinejoin="round" />
        <path d={accessory.inner} fill="none" stroke={accessory.ink} strokeWidth={accessory.innerWidth} strokeLinecap="round" strokeLinejoin="round" />
      </g>
    </svg>
  );
}
