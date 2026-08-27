export const PLAYGROUND_IMAGE_MIN_DIMENSION = 256;
export const PLAYGROUND_IMAGE_MAX_DIMENSION = 4096;
export const PLAYGROUND_IMAGE_DEFAULT_DIMENSION = 1024;
export const PLAYGROUND_IMAGE_SLIDER_STEP = 64;

export interface PlaygroundImageDimensions {
  width: number;
  height: number;
}

export function parsePlaygroundImageSize(
  value: string,
): PlaygroundImageDimensions | null {
  const match = /^(\d+)x(\d+)$/.exec(value.trim());
  if (!match) return null;

  const width = Number(match[1]);
  const height = Number(match[2]);
  if (
    !Number.isInteger(width) ||
    !Number.isInteger(height) ||
    width < PLAYGROUND_IMAGE_MIN_DIMENSION ||
    width > PLAYGROUND_IMAGE_MAX_DIMENSION ||
    height < PLAYGROUND_IMAGE_MIN_DIMENSION ||
    height > PLAYGROUND_IMAGE_MAX_DIMENSION
  ) {
    return null;
  }

  return { width, height };
}

export function formatPlaygroundImageSize(width: number, height: number): string {
  return `${width}x${height}`;
}

export function normalizePlaygroundImageSize(value: string): string {
  const dimensions = parsePlaygroundImageSize(value);
  return dimensions
    ? formatPlaygroundImageSize(dimensions.width, dimensions.height)
    : "";
}

export function clampPlaygroundImageDimension(value: number): number {
  return Math.min(
    PLAYGROUND_IMAGE_MAX_DIMENSION,
    Math.max(PLAYGROUND_IMAGE_MIN_DIMENSION, Math.round(value)),
  );
}
