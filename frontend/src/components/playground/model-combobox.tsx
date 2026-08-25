import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, CornerDownLeft, ImageIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import { ModelIcon } from "@/components/ModelIcon";
import { cn } from "@/lib/utils";
import type { ModelMetadataRecord } from "@/lib/api";

/** Substrings marking a model id as an image model (playground.spec.md PG-SEL4). */
const IMAGE_MODEL_HINTS = [
  "dall-e",
  "dalle",
  "gpt-image",
  "flux",
  "stable-diffusion",
  "sdxl",
  "sd3",
  "imagen",
  "seedream",
  "seededit",
  "kolors",
  "ideogram",
  "recraft",
  "cogview",
  "qwen-image",
  "hunyuan-image",
  "nano-banana",
  "janus",
  "hidream",
];

function isImageModelRecord(record: ModelMetadataRecord): boolean {
  if (record.mode && record.mode.toLowerCase().includes("image")) return true;
  const id = record.model_id.toLowerCase();
  return IMAGE_MODEL_HINTS.some((hint) => id.includes(hint));
}

const MAX_VISIBLE_OPTIONS = 100;

interface ModelComboboxProps {
  value: string;
  onChange: (modelId: string) => void;
  records: ModelMetadataRecord[];
  kind: "chat" | "image";
  isLoading: boolean;
  disabled?: boolean;
}

function ModelRow({
  record,
  selected,
  onSelect,
}: {
  record: ModelMetadataRecord;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  return (
    <CommandItem value={record.model_id} onSelect={() => onSelect(record.model_id)}>
      <ModelIcon
        model={record.model_id}
        provider={record.models_dev_provider}
        className="h-4 w-4 shrink-0"
      />
      <span className="min-w-0 flex-1 truncate font-mono text-xs">
        {record.model_id}
      </span>
      <Check
        className={cn("h-4 w-4 shrink-0", selected ? "opacity-100" : "opacity-0")}
      />
    </CommandItem>
  );
}

export function ModelCombobox({
  value,
  onChange,
  records,
  kind,
  isLoading,
  disabled,
}: ModelComboboxProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");

  const { imageRecords, otherRecords } = useMemo(() => {
    const query = search.trim().toLowerCase();
    const matches = query
      ? records.filter((r) => r.model_id.toLowerCase().includes(query))
      : records;
    if (kind === "image") {
      return {
        imageRecords: matches.filter(isImageModelRecord).slice(0, MAX_VISIBLE_OPTIONS),
        otherRecords: matches
          .filter((r) => !isImageModelRecord(r))
          .slice(0, MAX_VISIBLE_OPTIONS),
      };
    }
    return { imageRecords: [], otherRecords: matches.slice(0, MAX_VISIBLE_OPTIONS) };
  }, [records, search, kind]);

  const trimmedSearch = search.trim();
  const showCustomEntry =
    trimmedSearch.length > 0 &&
    !records.some((r) => r.model_id === trimmedSearch);

  if (isLoading) {
    return <Skeleton className="h-8 w-36 rounded-md" />;
  }

  const select = (id: string) => {
    onChange(id);
    setSearch("");
    setOpen(false);
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled}
          aria-label={t(kind === "image" ? "playground.imageModel" : "playground.chatModel")}
          className="h-8 max-w-[11rem] gap-1.5 border border-transparent px-2 text-xs font-medium text-muted-foreground hover:border-border hover:text-foreground sm:max-w-[14rem]"
        >
          {kind === "image" && !value ? (
            <ImageIcon className="h-3.5 w-3.5 shrink-0" />
          ) : (
            <ModelIcon model={value || "model"} className="h-3.5 w-3.5 shrink-0" />
          )}
          <span className="min-w-0 truncate font-mono">
            {value || t("playground.selectModel")}
          </span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-60" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-72 p-0" align="start">
        <Command shouldFilter={false}>
          <CommandInput
            placeholder={t("playground.searchModels")}
            value={search}
            onValueChange={setSearch}
          />
          <CommandList>
            {!showCustomEntry && (
              <CommandEmpty>{t("playground.noModels")}</CommandEmpty>
            )}
            {showCustomEntry && (
              <CommandGroup>
                <CommandItem
                  value={`custom:${trimmedSearch}`}
                  onSelect={() => select(trimmedSearch)}
                >
                  <CornerDownLeft className="h-4 w-4 shrink-0" />
                  <span className="min-w-0 flex-1 truncate text-xs">
                    {t("playground.useCustomModel", { id: trimmedSearch })}
                  </span>
                </CommandItem>
              </CommandGroup>
            )}
            {kind === "image" && imageRecords.length > 0 && (
              <CommandGroup heading={t("playground.imageModels")}>
                {imageRecords.map((record) => (
                  <ModelRow
                    key={record.model_id}
                    record={record}
                    selected={record.model_id === value}
                    onSelect={select}
                  />
                ))}
              </CommandGroup>
            )}
            {otherRecords.length > 0 && (
              <CommandGroup
                heading={kind === "image" ? t("playground.allModels") : undefined}
              >
                {otherRecords.map((record) => (
                  <ModelRow
                    key={record.model_id}
                    record={record}
                    selected={record.model_id === value}
                    onSelect={select}
                  />
                ))}
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
