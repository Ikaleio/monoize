const CARD_SPANS = [
  "md:col-span-2 lg:col-span-7",
  "lg:col-span-5",
  "lg:col-span-4",
  "lg:col-span-4",
  "lg:col-span-4",
  "md:col-span-2 lg:col-span-5",
  "md:col-span-2 lg:col-span-7",
] as const;

export function getModelCardSpan(index: number): string {
  return CARD_SPANS[index % CARD_SPANS.length];
}
