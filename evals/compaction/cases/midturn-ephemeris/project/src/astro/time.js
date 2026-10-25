// Time arguments shared by the astro modules.
export const J2000 = 2451545.0;
export function julianMillennia(jdTT) { return (jdTT - J2000) / 365250; }
export function julianCenturies(jdTT) { return (jdTT - J2000) / 36525; }
export function meanObliquity(T) { /* T in centuries; degrees */ return 23.439291111 - 0.0130041667 * T; }
