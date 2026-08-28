export interface PerformanceBrickRange {
  start: Date;
  end: Date;
}

export function performanceBrickRange(
  index: number,
  brickCount: number,
  timeFrom: string,
  timeTo: string,
): PerformanceBrickRange | null {
  const fromMs = Date.parse(timeFrom);
  const toMs = Date.parse(timeTo);
  if (
    !Number.isInteger(index) ||
    !Number.isInteger(brickCount) ||
    index < 0 ||
    brickCount <= 0 ||
    index >= brickCount ||
    !Number.isFinite(fromMs) ||
    !Number.isFinite(toMs) ||
    toMs <= fromMs
  ) {
    return null;
  }

  const intervalMs = (toMs - fromMs) / brickCount;
  return {
    start: new Date(fromMs + intervalMs * index),
    end: new Date(fromMs + intervalMs * (index + 1)),
  };
}

export function formatPerformanceBrickRange(
  index: number,
  brickCount: number,
  timeFrom: string,
  timeTo: string,
  locale: string,
): string | null {
  const range = performanceBrickRange(index, brickCount, timeFrom, timeTo);
  if (!range) return null;

  const formatter = new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
  return formatter.formatRange(range.start, range.end);
}
