import { useCallback, useState } from "react";

/** Preference keys persisted in localStorage (playground.spec.md PG-STATE2). */
export const PLAYGROUND_PREF_KEYS = {
  group: "playground_group",
  chatModel: "playground_chat_model",
  imageModel: "playground_image_model",
  imageSize: "playground_image_size",
  apiKeyId: "playground_api_key_id",
  temperature: "playground_temperature",
  maxTokens: "playground_max_tokens",
  systemPrompt: "playground_system_prompt",
} as const;

/** Legacy keys removed on mount (PG-STATE3) so no pasted secret survives. */
export const PLAYGROUND_LEGACY_KEYS = [
  "playground_api_key",
  "playground_model",
];

export interface PlaygroundPrefs {
  group: string;
  chatModel: string;
  imageModel: string;
  imageSize: string;
  apiKeyId: string;
  temperature: string;
  maxTokens: string;
  systemPrompt: string;
}

type PrefName = keyof PlaygroundPrefs;

function readPref(name: PrefName): string {
  try {
    return localStorage.getItem(PLAYGROUND_PREF_KEYS[name]) ?? "";
  } catch {
    return "";
  }
}

function writePref(name: PrefName, value: string) {
  try {
    if (value) {
      localStorage.setItem(PLAYGROUND_PREF_KEYS[name], value);
    } else {
      localStorage.removeItem(PLAYGROUND_PREF_KEYS[name]);
    }
  } catch {
    /* storage unavailable – prefs stay in memory */
  }
}

export function purgeLegacyPlaygroundKeys() {
  try {
    for (const key of PLAYGROUND_LEGACY_KEYS) {
      localStorage.removeItem(key);
    }
  } catch {
    /* ignore */
  }
}

export function usePlaygroundPrefs(): [
  PlaygroundPrefs,
  (name: PrefName, value: string) => void,
] {
  const [prefs, setPrefs] = useState<PlaygroundPrefs>(() => ({
    group: readPref("group"),
    chatModel: readPref("chatModel"),
    imageModel: readPref("imageModel"),
    imageSize: readPref("imageSize"),
    apiKeyId: readPref("apiKeyId"),
    temperature: readPref("temperature"),
    maxTokens: readPref("maxTokens"),
    systemPrompt: readPref("systemPrompt"),
  }));

  const setPref = useCallback((name: PrefName, value: string) => {
    writePref(name, value);
    setPrefs((prev) => (prev[name] === value ? prev : { ...prev, [name]: value }));
  }, []);

  return [prefs, setPref];
}
