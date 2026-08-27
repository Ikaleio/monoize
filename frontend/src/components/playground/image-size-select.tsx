import { useTranslation } from "react-i18next";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  normalizePlaygroundImageSize,
  PLAYGROUND_IMAGE_SIZES,
} from "./image-size";

interface ImageSizeSelectProps {
  value: string;
  onChange: (value: string) => void;
}

function imageSizeLabel(value: string): string {
  return value.replace("x", " × ");
}

export function ImageSizeSelect({ value, onChange }: ImageSizeSelectProps) {
  const { t } = useTranslation();
  const selectedValue = normalizePlaygroundImageSize(value) || "auto";

  return (
    <Select
      value={selectedValue}
      onValueChange={(nextValue) =>
        onChange(nextValue === "auto" ? "" : normalizePlaygroundImageSize(nextValue))
      }
    >
      <SelectTrigger
        aria-label={t("playground.imageSize")}
        className="h-8 w-auto max-w-[9rem] gap-1 border-transparent bg-transparent px-2 py-0 text-xs font-medium text-muted-foreground shadow-none hover:border-border hover:text-foreground"
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent side="top" sideOffset={8} align="start" collisionPadding={16}>
        <SelectGroup>
          <SelectItem value="auto">{t("playground.imageSizeAuto")}</SelectItem>
          {PLAYGROUND_IMAGE_SIZES.map((size) => (
            <SelectItem key={size} value={size}>
              {imageSizeLabel(size)}
            </SelectItem>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
}
