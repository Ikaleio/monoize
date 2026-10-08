/** Time display helpers shared by every dashboard surface (frontend-design-system.spec.md DS66). */

type TimeInput = string | number | Date;

const pad = (value: number) => String(value).padStart(2, "0");

function toDate(value: TimeInput): Date | null {
  const date = value instanceof Date ? value : new Date(value);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Formats a timestamp as local `YYYY-MM-DD HH:mm:ss`; invalid input renders `—`. */
export function formatDateTime(value: TimeInput): string {
  const date = toDate(value);
  if (!date) return "—";
  return `${formatDate(date)} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** Formats a calendar date as local `YYYY-MM-DD`; invalid input renders `—`. */
export function formatDate(value: TimeInput): string {
  const date = toDate(value);
  if (!date) return "—";
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}
