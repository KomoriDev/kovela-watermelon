const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

export function encodeBase64(bytes) {
  let output = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i], b = i + 1 < bytes.length ? bytes[i + 1] : 0, c = i + 2 < bytes.length ? bytes[i + 2] : 0;
    output += alphabet[a >>> 2] + alphabet[((a & 3) << 4) | (b >>> 4)];
    if (i + 1 < bytes.length) output += alphabet[((b & 15) << 2) | (c >>> 6)];
    if (i + 2 < bytes.length) output += alphabet[c & 63];
  }
  return output;
}

export function decodeBase64(value) {
  if (!value || value.length % 4 === 1 || !/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("Invalid image encoding");
  const bytes = new Uint8Array(Math.floor(value.length * 6 / 8));
  let buffer = 0, bits = 0, offset = 0;
  for (let i = 0; i < value.length; i += 1) {
    buffer = (buffer << 6) | alphabet.indexOf(value[i]);
    bits += 6;
    if (bits >= 8) { bits -= 8; bytes[offset++] = (buffer >>> bits) & 255; }
  }
  if ((buffer & ((1 << bits) - 1)) !== 0) throw new Error("Non-canonical image encoding");
  return bytes;
}
