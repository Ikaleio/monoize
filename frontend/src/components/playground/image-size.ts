export const PLAYGROUND_IMAGE_SIZES = [
  "1024x1024",
  "1024x1536",
  "1536x1024",
] as const;

export type PlaygroundImageSize = (typeof PLAYGROUND_IMAGE_SIZES)[number] | "";

export function normalizePlaygroundImageSize(value: string): PlaygroundImageSize {
  return PLAYGROUND_IMAGE_SIZES.some((size) => size === value)
    ? (value as PlaygroundImageSize)
    : "";
}
