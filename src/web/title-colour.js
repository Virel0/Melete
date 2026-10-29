// A title's strongest colour, measured from its art in the browser: the most prominent hue,
// not the average (which turns to mud), as Mneme takes it. The page fades from it down to
// its ground, and Play wears it, lightened until its dark words read on it.
//
// - strongest(src) → a promise of [r, g, b], or null when the art has no colour to speak of.
// - tintPage(rgb): the fade down the page, until the page is left.
// - buttonColour(rgb) → [r, g, b] for a button with the stage's dark ink on it.

import { leaving } from "/web/page-state.js";

const SIZE = 32;
const BINS = 24;

// RGB (0–255) → [hue 0–1, saturation, lightness].
function hsl([r, g, b]) {
  const [R, G, B] = [r / 255, g / 255, b / 255];
  const max = Math.max(R, G, B);
  const min = Math.min(R, G, B);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  const h = max === R ? (G - B) / d + (G < B ? 6 : 0) : max === G ? (B - R) / d + 2 : (R - G) / d + 4;
  return [h / 6, s, l];
}

// The hue with the most colourful pixels behind it, as the mean of those pixels.
function prominent(data) {
  const weight = new Array(BINS).fill(0);
  const sums = Array.from({ length: BINS }, () => [0, 0, 0]);
  for (let i = 0; i < data.length; i += 4) {
    const rgb = [data[i], data[i + 1], data[i + 2]];
    const [h, s, l] = hsl(rgb);
    // Greys, near-black and near-white say nothing about the title's colour.
    if (s < 0.2 || l < 0.12 || l > 0.88) continue;
    const bin = Math.min(BINS - 1, Math.floor(h * BINS));
    const w = s * (1 - Math.abs(l - 0.5));
    weight[bin] += w;
    for (let c = 0; c < 3; c++) sums[bin][c] += rgb[c] * w;
  }
  const best = weight.indexOf(Math.max(...weight));
  // Too little colour to call it the title's.
  if (weight[best] < (SIZE * SIZE) / 40) return null;
  return sums[best].map((v) => Math.round(v / weight[best]));
}

export function strongest(src) {
  if (!src) return Promise.resolve(null);
  return new Promise((resolve) => {
    const img = new Image();
    img.addEventListener("load", () => {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = SIZE;
      const g = canvas.getContext("2d", { willReadFrequently: true });
      g.drawImage(img, 0, 0, SIZE, SIZE);
      try {
        resolve(prominent(g.getImageData(0, 0, SIZE, SIZE).data));
      } catch {
        resolve(null);
      }
    });
    img.addEventListener("error", () => resolve(null));
    img.src = src;
  });
}

// "rgb(r g b)" for CSS.
export const css = (rgb) => `rgb(${rgb.join(" ")})`;

export function tintPage(rgb) {
  const root = document.documentElement;
  root.style.setProperty("--title-tint", css(rgb));
  root.classList.add("title-tinted");
  leaving.push(() => {
    root.classList.remove("title-tinted");
    root.style.removeProperty("--title-tint");
  });
}

// Relative luminance, as WCAG works out contrast.
function luminance(rgb) {
  const [r, g, b] = rgb.map((v) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrast(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

// `rgb` lightened toward white until `ink` reads on it at 4.5:1 or more.
export function buttonColour(rgb, ink) {
  let colour = rgb;
  for (let step = 0; step <= 20 && contrast(colour, ink) < 4.5; step++) {
    colour = rgb.map((v) => Math.round(v + ((255 - v) * step) / 20));
  }
  return colour;
}
