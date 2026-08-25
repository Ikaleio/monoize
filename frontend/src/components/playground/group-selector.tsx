import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Layers } from "lucide-react";
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
import { cn } from "@/lib/utils";
import type { Group } from "@/lib/api";

interface GroupSelectorProps {
  value: string;
  onChange: (groupId: string) => void;
  groups: Group[];
  isLoading: boolean;
  disabled?: boolean;
}

export function GroupSelector({
  value,
  onChange,
  groups,
  isLoading,
  disabled,
}: GroupSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const selectedGroup = useMemo(
    () => groups.find((group) => group.id === value),
    [groups, value],
  );

  if (isLoading) {
    return <Skeleton className="h-8 w-24 rounded-md" />;
  }

  const select = (groupId: string) => {
    onChange(groupId);
    setOpen(false);
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          disabled={disabled}
          aria-label={t("playground.group")}
          className="h-8 max-w-[9rem] gap-1.5 border border-transparent px-2 text-xs font-medium text-muted-foreground hover:border-border hover:text-foreground"
        >
          <Layers className="h-3.5 w-3.5 shrink-0" />
          <span className="min-w-0 truncate">
            {selectedGroup?.name || t("playground.groupAuto")}
          </span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-60" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-56 p-0" align="start">
        <Command>
          <CommandInput placeholder={t("playground.searchGroups")} />
          <CommandList>
            <CommandEmpty>{t("playground.noGroups")}</CommandEmpty>
            <CommandGroup>
              <CommandItem value="__auto__" onSelect={() => select("")}>
                <span className="min-w-0 flex-1 truncate text-xs">
                  {t("playground.groupAuto")}
                </span>
                <Check
                  className={cn("h-4 w-4", value === "" ? "opacity-100" : "opacity-0")}
                />
              </CommandItem>
              {groups.map((group) => (
                <CommandItem
                  key={group.id}
                  value={group.id}
                  keywords={[group.name]}
                  onSelect={() => select(group.id)}
                >
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">
                    {group.name}
                  </span>
                  <Check
                    className={cn(
                      "h-4 w-4",
                      value === group.id ? "opacity-100" : "opacity-0",
                    )}
                  />
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
