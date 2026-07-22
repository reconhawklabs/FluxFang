// Shared helpers for the sensor encryption key: a client-side generator and a
// format validator. The key is 32 random bytes, base64-encoded; the backend
// treats a well-formed key as one that base64-decodes to exactly 32 bytes.

/** 32 random bytes, base64-encoded — a client-side convenience generator for
 * the shared encryption key. */
export function generateKey(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

/** True iff `s` base64-decodes to exactly 32 bytes — the shape the backend
 * accepts for a sensor encryption key. */
export function isValidKey(s: string): boolean {
  try {
    return atob(s).length === 32;
  } catch {
    return false;
  }
}
