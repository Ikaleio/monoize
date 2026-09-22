export const PLAYGROUND_IMAGE_QUALITIES = [
  "default",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

export type PlaygroundImageQuality = (typeof PLAYGROUND_IMAGE_QUALITIES)[number];

export function normalizePlaygroundImageQuality(value: string): PlaygroundImageQuality {
  return PLAYGROUND_IMAGE_QUALITIES.find((quality) => quality === value) ?? "default";
}
