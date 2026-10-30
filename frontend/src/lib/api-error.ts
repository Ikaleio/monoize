import type { TFunction } from "i18next";
import { toast } from "sonner";
import { DashboardApiError } from "@/lib/api";

/**
 * Localized message for a failed request: `apiErrors.<code>` when that key exists,
 * otherwise `common.requestFailed`. The raw server message is never shown.
 */
export function apiErrorMessage(error: unknown, t: TFunction): string {
  if (error instanceof DashboardApiError) {
    const key = `apiErrors.${error.code}`;
    if (t(key, { defaultValue: "" }) !== "") return t(key);
  }
  return t("common.requestFailed");
}

/** Shows one error toast for a request failure that is not tied to a form field. */
export function toastApiError(error: unknown, t: TFunction, title?: string): void {
  const message = apiErrorMessage(error, t);
  if (title) {
    toast.error(title, { description: message });
  } else {
    toast.error(message);
  }
}

/** Request field named by the backend error, used to place the error on that field. */
export function apiErrorParam(error: unknown): string | null {
  return error instanceof DashboardApiError ? error.param : null;
}
