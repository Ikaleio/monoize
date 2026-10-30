/**
 * Client approximation of the Rust `regex` crate (SSU-20d). The backend stays
 * authoritative, so this only has to avoid rejecting common Rust syntax that
 * `RegExp` lacks and to catch constructs that `RegExp` accepts but Rust rejects.
 */
const RUST_REJECTED = /\(\?<?[=!]|\\[1-9]/;

export function isRedirectPatternInvalid(pattern: string): boolean {
  if (!pattern.trim()) return false;
  if (RUST_REJECTED.test(pattern)) return true;
  const translated = pattern
    .replace(/\(\?[imsUux-]+\)/g, "")
    .replace(/\(\?[imsUux-]+:/g, "(?:")
    .replace(/\(\?P</g, "(?<");
  try {
    new RegExp(translated);
    return false;
  } catch {
    return true;
  }
}
