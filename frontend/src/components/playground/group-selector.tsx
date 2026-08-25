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

interface GroupSelectorProps {
  value: string;
  onChange: (group: string) => void;
  groups: string[];
  /** Non-empty restricts selectable groups to the session user's scope (PG-SEL2). */
  userAllowedGroups: string[];
  isLoading: boolean;
  disabled?: boolean;
}

export function GroupSelector({
  value,
  onChange,
  groups,
  userAllowedGroups,
  isLoading,
  disabled,
}: GroupSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const options = useMemo(() => {
    if (userAllowedGroups.length === 0) return groups;
    return groups.filter((group) => userAllowedGroups.includes(group));
  }, [groups, userAllowedGroups]);

  if (isLoading) {
    return <Skeleton className="h-8 w-24 rounded-md" />;
  }

  const select = (group: string) => {
    onChange(group);
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
            {value || t("playground.groupAuto")}
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
              {options.map((group) => (
                <CommandItem key={group} value={group} onSelect={() => select(group)}>
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">
                    {group}
                  </span>
                  <Check
                    className={cn(
                      "h-4 w-4",
                      value === group ? "opacity-100" : "opacity-0",
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
