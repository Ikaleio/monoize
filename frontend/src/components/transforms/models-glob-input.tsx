import { useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

type ModelsGlobInputProps = {
  value: string[] | null | undefined;
  onChange: (value: string[] | null) => void;
  disabled?: boolean;
};

export function ModelsGlobInput({ value, onChange, disabled }: ModelsGlobInputProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  const patterns = value ?? [];

  const removePattern = (pattern: string) => {
    const next = patterns.filter((entry) => entry !== pattern);
    onChange(next.length === 0 ? null : next);
  };

  const flushDraft = () => {
    const parts = draft
      .split(",")
      .map((entry) => entry.trim())
      .filter(Boolean);
    if (parts.length > 0) {
      const next = [...patterns];
      for (const part of parts) {
        if (!next.includes(part)) {
          next.push(part);
        }
      }
      onChange(next.length === 0 ? null : next);
    }
    setDraft("");
  };

  return (
    <div className="space-y-2">
      <Input
        value={draft}
        disabled={disabled}
        placeholder={t("transforms.modelsPlaceholder")}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={flushDraft}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            flushDraft();
          }
        }}
      />
      {patterns.length > 0 ? (
        <div className="flex flex-wrap gap-2">
          {patterns.map((pattern) => (
            <Badge key={pattern} variant="secondary" className="flex items-center gap-1 font-mono">
              <span>{pattern}</span>
              {!disabled && (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-4 w-4"
                  onClick={() => removePattern(pattern)}
                >
                  <X className="h-3 w-3" />
                </Button>
              )}
            </Badge>
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">{t("transforms.modelsAllModelsHint")}</p>
      )}
      <p className="text-xs text-muted-foreground">{t("transforms.modelsGlobHint")}</p>
    </div>
  );
}
