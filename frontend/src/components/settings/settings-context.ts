import { createContext, useContext } from "react";

export interface SettingsFieldError {
  entryId: string;
  message: string;
  /** Draft row of `global_model_redirects` the error belongs to. */
  redirectIndex?: number;
}

export interface SettingsRowState {
  /** Entry currently highlighted after a jump (SSU-9b). */
  highlightId: string | null;
  /** Error reported by the last save attempt (SSU-22). */
  fieldError: SettingsFieldError | null;
  reportValidity: (entryId: string, valid: boolean) => void;
}

export const SettingsRowStateContext = createContext<SettingsRowState>({
  highlightId: null,
  fieldError: null,
  reportValidity: () => undefined,
});

export function useSettingsRowState(): SettingsRowState {
  return useContext(SettingsRowStateContext);
}
