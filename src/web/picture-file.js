// A picture chosen to upload, read before it's sent: its bytes, so a browser that loses sight
// of the file once it's chosen (a sandboxed one) says so at once rather than failing mid-send,
// and its type from those bytes, so a PNG named ".jpg" goes as the PNG it is.

export const PICTURE_TYPES = ["image/jpeg", "image/png", "image/webp"];
export const PICTURE_MAX = 5 * 1024 * 1024;

// The type the bytes start as, or null for none of the three.
function typeOf(bytes) {
  const starts = (...b) => b.every((v, i) => bytes[i] === v);
  if (starts(0xff, 0xd8, 0xff)) return "image/jpeg";
  if (starts(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)) return "image/png";
  const text = (from, to) => String.fromCharCode(...bytes.slice(from, to));
  if (bytes.length >= 12 && text(0, 4) === "RIFF" && text(8, 12) === "WEBP") return "image/webp";
  return null;
}

// { blob } to send, or { error } to show.
export async function readPicture(file) {
  if (file.size > PICTURE_MAX) return { error: "That picture is over 5 MB. Choose a smaller one." };
  let bytes;
  try {
    bytes = new Uint8Array(await file.arrayBuffer());
  } catch {
    return { error: "This browser couldn't read that file. A browser that runs sandboxed may only reach some folders: try one from Pictures or Downloads." };
  }
  const type = typeOf(bytes);
  if (!type) return { error: "That file isn't a JPEG, PNG or WebP picture." };
  return { blob: new Blob([bytes], { type }) };
}
