import { Fragment, useMemo, useState } from "react";
import { Bot, RefreshCw, SearchX, X } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";

interface CodexModelSelectorProps {
  availableModelIds: string[];
  selectedModelIds: string[];
  isLoading: boolean;
  loadError?: unknown;
  onRetry: () => void;
  onChange: (modelIds: string[]) => void;
}

function uniqueModelIds(modelIds: string[]) {
  return Array.from(new Set(modelIds.map((modelId) => modelId.trim()).filter(Boolean)));
}

export function CodexModelSelector({
  availableModelIds,
  selectedModelIds,
  isLoading,
  loadError,
  onRetry,
  onChange,
}: CodexModelSelectorProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const selected = useMemo(() => uniqueModelIds(selectedModelIds), [selectedModelIds]);
  const available = useMemo(() => uniqueModelIds(availableModelIds).sort(), [availableModelIds]);
  const availableSet = useMemo(() => new Set(available), [available]);
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const modelIds = useMemo(
    () => [...selected, ...available.filter((modelId) => !selectedSet.has(modelId))],
    [available, selected, selectedSet]
  );
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const filteredModelIds = modelIds.filter((modelId) =>
    modelId.toLocaleLowerCase().includes(normalizedQuery)
  );

  const toggleModel = (modelId: string, checked: boolean) => {
    onChange(
      checked
        ? [...selected, modelId]
        : selected.filter((selectedModelId) => selectedModelId !== modelId)
    );
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="secondary">
            {t("settings.codexModelsSelected", { count: selected.length })}
          </Badge>
          <span className="text-sm text-muted-foreground">
            {t("settings.codexModelsAvailable", { count: available.length })}
          </span>
        </div>
        {selected.length > 0 ? (
          <Button type="button" variant="ghost" size="sm" onClick={() => onChange([])}>
            <X data-icon="inline-start" />
            {t("settings.codexModelsClear")}
          </Button>
        ) : null}
      </div>

      <Field>
        <FieldLabel htmlFor="codex-model-search" className="sr-only">
          {t("settings.codexModelsSearch")}
        </FieldLabel>
        <Input
          id="codex-model-search"
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("settings.codexModelsSearch")}
          autoComplete="off"
        />
      </Field>

      {loadError ? (
        <Alert variant="destructive">
          <AlertTitle>{t("settings.codexModelsLoadFailed")}</AlertTitle>
          <AlertDescription className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <p>{t("settings.codexModelsLoadFailedDescription")}</p>
            <Button type="button" variant="outline" size="sm" onClick={onRetry}>
              <RefreshCw data-icon="inline-start" />
              {t("settings.codexModelsRetry")}
            </Button>
          </AlertDescription>
        </Alert>
      ) : null}

      {isLoading ? (
        <div className="flex flex-col gap-2" aria-busy="true" aria-label={t("common.loading")}>
          {Array.from({ length: 5 }, (_, index) => (
            <Skeleton key={index} className="h-11 w-full" />
          ))}
        </div>
      ) : filteredModelIds.length > 0 ? (
        <div className="max-h-96 overflow-y-auto rounded-md border">
          <FieldSet>
            <FieldLegend className="sr-only">{t("settings.codexModelsLegend")}</FieldLegend>
            <FieldGroup data-slot="checkbox-group" className="data-[slot=checkbox-group]:gap-0">
              {filteredModelIds.map((modelId, index) => {
                const checkboxId = `codex-model-${index}`;
                const isAvailable = availableSet.has(modelId);
                return (
                  <Fragment key={modelId}>
                    <Field orientation="horizontal" className="p-3">
                      <Checkbox
                        id={checkboxId}
                        checked={selectedSet.has(modelId)}
                        onCheckedChange={(checked) => toggleModel(modelId, checked === true)}
                      />
                      <FieldLabel
                        htmlFor={checkboxId}
                        className="min-w-0 flex-1 cursor-pointer items-center justify-between"
                      >
                        <span className="truncate font-mono text-sm">{modelId}</span>
                        {!isAvailable ? (
                          <Badge variant="outline">{t("settings.codexModelsUnavailable")}</Badge>
                        ) : null}
                      </FieldLabel>
                    </Field>
                    {index < filteredModelIds.length - 1 ? <Separator /> : null}
                  </Fragment>
                );
              })}
            </FieldGroup>
          </FieldSet>
        </div>
      ) : (
        <EmptyState
          variant="inline"
          icon={normalizedQuery ? <SearchX className="size-5" /> : <Bot className="size-5" />}
          title={
            normalizedQuery
              ? t("settings.codexModelsNoMatch")
              : t("settings.codexModelsEmpty")
          }
          description={
            normalizedQuery
              ? t("settings.codexModelsNoMatchDescription")
              : t("settings.codexModelsEmptyDescription")
          }
        />
      )}

      <p className="text-pretty text-sm leading-6 text-muted-foreground">
        {t("settings.codexModelsCompatibilityHelp")}
      </p>
    </div>
  );
}
