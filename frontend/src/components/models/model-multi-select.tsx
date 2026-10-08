import { useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot, Plus, SearchX, X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { QueryError } from "@/components/ui/query-error";
import { Skeleton } from "@/components/ui/skeleton";

interface ModelMultiSelectProps {
  /** Selected exact model IDs, in stored order. */
  value: string[];
  /** Model IDs that can currently be routed. */
  options: string[];
  loading?: boolean;
  error?: unknown;
  onRetry?: () => unknown;
  onChange: (value: string[]) => void;
  /** Allows adding a typed ID that is not in `options`. */
  allowCustom?: boolean;
  /** Accessible name of the option list. */
  label: string;
}

/** Searchable checkbox list for a set of exact model IDs (frontend-design-system.spec.md DS63). */
export function ModelMultiSelect({
  value,
  options,
  loading = false,
  error,
  onRetry,
  onChange,
  allowCustom = false,
  label,
}: ModelMultiSelectProps) {
  const { t } = useTranslation();
  const idPrefix = useId();
  const [query, setQuery] = useState("");

  const selected = useMemo(
    () => Array.from(new Set(value.map((id) => id.trim()).filter(Boolean))),
    [value],
  );
  const available = useMemo(
    () => Array.from(new Set(options.map((id) => id.trim()).filter(Boolean))).sort(),
    [options],
  );
  const availableSet = useMemo(() => new Set(available), [available]);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const rows = useMemo(
    () => [...selected, ...available.filter((id) => !selectedSet.has(id))],
    [available, selected, selectedSet],
  );

  const needle = query.trim();
  const loweredNeedle = needle.toLocaleLowerCase();
  const filtered = rows.filter((id) => id.toLocaleLowerCase().includes(loweredNeedle));
  const customCandidate = allowCustom && needle !== "" && !rows.includes(needle) ? needle : null;
  const blockingError = error != null && available.length === 0;

  const toggle = (id: string, checked: boolean) =>
    onChange(checked ? [...selected, id] : selected.filter((selectedId) => selectedId !== id));

  const addCustom = () => {
    if (!customCandidate) return;
    onChange([...selected, customCandidate]);
    setQuery("");
  };

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-sm tabular-nums text-muted-foreground">
          {t("modelSelect.summary", { selected: selected.length, available: available.length })}
        </span>
        {selected.length > 0 ? (
          <Button type="button" variant="ghost" size="sm" onClick={() => onChange([])}>
            <X data-icon="inline-start" aria-hidden="true" />
            {t("modelSelect.clear")}
          </Button>
        ) : null}
      </div>

      <Input
        type="search"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={(event) => {
          // Enter adds the typed ID instead of submitting the surrounding form.
          if (event.key === "Enter" && customCandidate) {
            event.preventDefault();
            addCustom();
          }
        }}
        placeholder={t("modelSelect.search")}
        aria-label={t("modelSelect.search")}
        autoComplete="off"
      />

      {error != null && onRetry ? (
        <QueryError onRetry={onRetry} stale={!blockingError} />
      ) : null}

      {loading ? (
        <div className="flex flex-col gap-2" aria-busy="true" aria-label={t("common.loading")}>
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="h-9 w-full" />
          ))}
        </div>
      ) : blockingError && selected.length === 0 ? null : filtered.length > 0 || customCandidate ? (
        <div
          role="group"
          aria-label={label}
          className="max-h-64 divide-y overflow-y-auto rounded-md border"
        >
          {customCandidate ? (
            <button
              type="button"
              onClick={addCustom}
              className="flex w-full items-center gap-3 px-3 py-2 text-left text-sm hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
            >
              <Plus className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
              <span className="min-w-0 truncate">
                {t("modelSelect.addCustom", { model: customCandidate })}
              </span>
            </button>
          ) : null}
          {filtered.map((id, index) => {
            const checkboxId = `${idPrefix}-${index}`;
            return (
              <div key={id} className="flex items-center gap-3 px-3 py-2 hover:bg-muted/50">
                <Checkbox
                  id={checkboxId}
                  checked={selectedSet.has(id)}
                  onCheckedChange={(checked) => toggle(id, checked === true)}
                />
                <label
                  htmlFor={checkboxId}
                  className="min-w-0 flex-1 cursor-pointer truncate font-mono text-sm"
                >
                  {id}
                </label>
                {!availableSet.has(id) && !blockingError ? (
                  <Badge variant="outline" className="shrink-0">
                    {t("common.unavailable")}
                  </Badge>
                ) : null}
              </div>
            );
          })}
        </div>
      ) : (
        <EmptyState
          variant="inline"
          icon={needle ? <SearchX className="size-5" /> : <Bot className="size-5" />}
          title={t(needle ? "modelSelect.noMatch" : "modelSelect.empty")}
          description={t(needle ? "modelSelect.noMatchDescription" : "modelSelect.emptyDescription")}
        />
      )}
    </div>
  );
}
