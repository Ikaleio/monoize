export async function loadPlaygroundImage(
  url: string,
  signal?: AbortSignal,
): Promise<Blob> {
  signal?.throwIfAborted();
  if (url.startsWith("data:")) {
    const separator = url.indexOf(",");
    if (separator < 0) throw new Error("invalid image data URL");
    const metadata = url.slice(5, separator);
    const data = decodeURIComponent(url.slice(separator + 1));
    const bytes = metadata.endsWith(";base64")
      ? Uint8Array.from(atob(data), (character) => character.charCodeAt(0))
      : new TextEncoder().encode(data);
    return new Blob([bytes], { type: metadata.split(";")[0] || "image/png" });
  }
  const response = await fetch(url, { signal });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.blob();
}
