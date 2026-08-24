import { useEffect, useRef, useState } from "react";
import type { CapSolveEvent, CapWidget } from "cap-widget";
import { Skeleton } from "@/components/ui/skeleton";
import { loadCapWidget } from "@/lib/cap-widget";

interface CapCaptchaProps {
  apiEndpoint: string;
  language: string;
  resetKey: number;
  onTokenChange: (token: string) => void;
  onError: () => void;
}

function capLanguage(language: string) {
  if (language === "zh") return "zh-cn";
  if (language === "zh-TW") return "zh-tw";
  return language;
}

export function CapCaptcha({
  apiEndpoint,
  language,
  resetKey,
  onTokenChange,
  onError,
}: CapCaptchaProps) {
  const widgetRef = useRef<CapWidget | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let active = true;
    loadCapWidget()
      .then(() => {
        if (active) setReady(true);
      })
      .catch(() => {
        if (active) onError();
      });
    return () => {
      active = false;
    };
  }, [onError]);

  useEffect(() => {
    const widget = widgetRef.current;
    if (!ready || !widget) return;
    const handleSolve = (event: CapSolveEvent) => onTokenChange(event.detail.token);
    const handleReset = () => onTokenChange("");
    const handleError = () => {
      onTokenChange("");
      onError();
    };
    widget.addEventListener("solve", handleSolve);
    widget.addEventListener("reset", handleReset);
    widget.addEventListener("error", handleError);
    return () => {
      widget.removeEventListener("solve", handleSolve);
      widget.removeEventListener("reset", handleReset);
      widget.removeEventListener("error", handleError);
    };
  }, [onError, onTokenChange, ready]);

  useEffect(() => {
    if (ready) widgetRef.current?.reset();
  }, [ready, resetKey]);

  if (!ready) {
    return <Skeleton className="h-12 w-full rounded-md" />;
  }

  return (
    <cap-widget
      ref={widgetRef}
      required
      data-cap-api-endpoint={apiEndpoint}
      data-cap-lang={capLanguage(language)}
      className="monoize-cap-widget"
    />
  );
}
