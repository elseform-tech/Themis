/** Whether text looks like it holds a credential. Runs on the device and keeps nothing: no
 *  logging of the text, the match or a hash. Same rules as `crates/core/src/secret_look.rs`;
 *  `secretSamples.json` holds the cases both must agree on. */

/** Characters scanned: a key further into a very long message is missed. */
const MAX_SCAN_CHARS = 256 * 1024;
const MIN_VALUE_CHARS = 20;
/** Bits per character below which a value reads as words or a repeated placeholder. */
const MIN_ENTROPY = 3.2;

const FIXED_SHAPES = [
  /(?:^|[^A-Za-z0-9_-])(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}/,
  /(?:^|[^A-Za-z0-9_-])github_pat_[A-Za-z0-9_]{30,}/,
  /(?:^|[^A-Za-z0-9_-])xox[baprs]-[0-9]+-[A-Za-z0-9-]{10,}/,
  /(?:^|[^A-Za-z0-9_-])AKIA[0-9A-Z]{16}(?:$|[^A-Za-z0-9])/,
  /(?:^|[^A-Za-z0-9_-])AIza[0-9A-Za-z_-]{35}(?:$|[^A-Za-z0-9_-])/,
  /-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----/,
  /(?:^|[^A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/,
];
const OPENAI_STYLE = /(?:^|[^A-Za-z0-9_-])sk-([A-Za-z0-9_-]{20,})/g;
const NAMED_VALUE = /(?:^|[^A-Za-z0-9_])[A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL)[A-Z0-9_]*[ \t]*[=:][ \t]*["']?([A-Za-z0-9+/_.=-]{20,})/g;
const UUID_SHAPE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

function entropy(text: string): number {
  const counts = new Map<string, number>();
  for (const c of text) counts.set(c, (counts.get(c) ?? 0) + 1);
  let bits = 0;
  for (const count of counts.values()) {
    const share = count / text.length;
    bits -= share * Math.log2(share);
  }
  return bits;
}

/** Both letters and digits, varied enough not to be words or a placeholder. */
function looksRandom(value: string): boolean {
  return value.length >= MIN_VALUE_CHARS && /[0-9]/.test(value) && /[A-Za-z]/.test(value) && entropy(value) >= MIN_ENTROPY && !UUID_SHAPE.test(value);
}

export function looksLikeSecret(input: string): boolean {
  const text = input.length > MAX_SCAN_CHARS ? input.slice(0, MAX_SCAN_CHARS) : input;
  if (FIXED_SHAPES.some(shape => shape.test(text))) return true;
  // `sk-` also starts CSS class names and the like: one long random part is what a key has.
  for (const found of text.matchAll(OPENAI_STYLE)) {
    if (looksRandom(found[1]) && found[1].split("-").some(part => part.length >= 16)) return true;
  }
  for (const found of text.matchAll(NAMED_VALUE)) {
    if (looksRandom(found[1])) return true;
  }
  return false;
}
