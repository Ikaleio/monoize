import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Separator } from "@/components/ui/separator";
import {
  normalizePlaygroundImageQuality,
  PLAYGROUND_IMAGE_QUALITIES,
} from "./image-quality";
import {
  clampPlaygroundImageDimension,
  formatPlaygroundImageSize,
  parsePlaygroundImageSize,
  PLAYGROUND_IMAGE_DEFAULT_DIMENSION,
  PLAYGROUND_IMAGE_MAX_DIMENSION,
  PLAYGROUND_IMAGE_MIN_DIMENSION,
  PLAYGROUND_IMAGE_SLIDER_STEP,
} from "./image-size";

interface ImageSizeControlProps {
  value: string;
  onChange: (value: string) => void;
  quality: string;
  onQualityChange: (value: string) => void;
}

type Dimension = "width" | "height";

const LANDSCAPE_PRESETS = [
  { width: 1024, height: 768, ratio: "4:3" },
  { width: 1280, height: 720, ratio: "16:9" },
  { width: 1536, height: 1024, ratio: "3:2" },
  { width: 1792, height: 1024, ratio: "7:4" },
  { width: 1920, height: 1080, ratio: "16:9" },
  { width: 2560, height: 1440, ratio: "16:9" },
  { width: 3840, height: 2160, ratio: "16:9" },
];

const SIZE_PRESET_GROUPS = [
  {
    label: "playground.imageSizeSquare",
    sizes: [512, 1024, 2048, 4096].map((size) => ({
      width: size,
      height: size,
      ratio: "1:1",
    })),
  },
  { label: "playground.imageSizeLandscape", sizes: LANDSCAPE_PRESETS },
  {
    label: "playground.imageSizePortrait",
    sizes: LANDSCAPE_PRESETS.map(({ width, height, ratio }) => ({
      width: height,
      height: width,
      ratio: ratio.split(":").reverse().join(":"),
    })),
  },
];

interface ImageSizeFieldsProps {
  width: number;
  height: number;
  onChange: (value: string) => void;
}

function ImageSizeFields({ width, height, onChange }: ImageSizeFieldsProps) {
  const { t } = useTranslation();
  const [widthInput, setWidthInput] = useState<string | null>(null);
  const [heightInput, setHeightInput] = useState<string | null>(null);

  const updateSize = (dimension: Dimension, nextValue: number) => {
    const nextWidth = dimension === "width" ? nextValue : width;
    const nextHeight = dimension === "height" ? nextValue : height;
    onChange(formatPlaygroundImageSize(nextWidth, nextHeight));
  };

  const updateInput = (dimension: Dimension, rawValue: string) => {
    if (dimension === "width") setWidthInput(rawValue);
    else setHeightInput(rawValue);

    const numberValue = rawValue.trim() ? Number(rawValue) : Number.NaN;
    if (
      Number.isInteger(numberValue) &&
      numberValue >= PLAYGROUND_IMAGE_MIN_DIMENSION &&
      numberValue <= PLAYGROUND_IMAGE_MAX_DIMENSION
    ) {
      updateSize(dimension, numberValue);
    }
  };

  const commitInput = (dimension: Dimension, rawValue: string) => {
    const fallback = dimension === "width" ? width : height;
    const numberValue = rawValue.trim() ? Number(rawValue) : Number.NaN;
    const nextValue = Number.isFinite(numberValue)
      ? clampPlaygroundImageDimension(numberValue)
      : fallback;

    if (dimension === "width") setWidthInput(null);
    else setHeightInput(null);
    if (nextValue !== fallback) updateSize(dimension, nextValue);
  };

  const renderDimension = (dimension: Dimension, label: string) => {
    const currentValue = dimension === "width" ? width : height;
    const inputValue = dimension === "width" ? widthInput : heightInput;
    const inputId = `playground-image-${dimension}`;

    return (
      <Field className="gap-2">
        <div className="flex items-center justify-between gap-3">
          <FieldLabel htmlFor={inputId}>{label}</FieldLabel>
          <div className="flex items-center gap-1.5">
            <Input
              id={inputId}
              type="number"
              inputMode="numeric"
              min={PLAYGROUND_IMAGE_MIN_DIMENSION}
              max={PLAYGROUND_IMAGE_MAX_DIMENSION}
              step={1}
              value={inputValue ?? String(currentValue)}
              onChange={(event) => updateInput(dimension, event.target.value)}
              onBlur={(event) => commitInput(dimension, event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
              }}
              className="h-8 w-24 px-2 text-right tabular-nums"
              aria-label={`${label} (${t("playground.imageSizePixels")})`}
            />
            <span className="text-xs text-muted-foreground">
              {t("playground.imageSizePixels")}
            </span>
          </div>
        </div>
        <Slider
          value={[currentValue]}
          min={PLAYGROUND_IMAGE_MIN_DIMENSION}
          max={PLAYGROUND_IMAGE_MAX_DIMENSION}
          step={PLAYGROUND_IMAGE_SLIDER_STEP}
          onValueChange={([nextValue]) => {
            if (dimension === "width") setWidthInput(null);
            else setHeightInput(null);
            updateSize(dimension, nextValue);
          }}
          aria-label={label}
        />
      </Field>
    );
  };

  return (
    <FieldGroup className="gap-5">
      {renderDimension("width", t("playground.imageSizeWidth"))}
      {renderDimension("height", t("playground.imageSizeHeight"))}
    </FieldGroup>
  );
}

export function ImageSizeControl({
  value,
  onChange,
  quality,
  onQualityChange,
}: ImageSizeControlProps) {
  const { t } = useTranslation();
  const parsed = parsePlaygroundImageSize(value);
  const width = parsed?.width ?? PLAYGROUND_IMAGE_DEFAULT_DIMENSION;
  const height = parsed?.height ?? PLAYGROUND_IMAGE_DEFAULT_DIMENSION;
  const sizeLabel = parsed
    ? `${parsed.width} × ${parsed.height}`
    : t("playground.imageSizeAuto");

  return (
    <div>
      <div className="mb-4 flex items-center justify-between gap-3">
        <p className="text-sm font-medium">{t("playground.imageSize")}</p>
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="outline"
              size="sm"
              aria-label={`${t("playground.imageSize")}: ${sizeLabel}`}
              className="h-8 gap-1.5 px-2 tabular-nums"
            >
              {sizeLabel}
              <ChevronDown data-icon="inline-end" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            collisionPadding={16}
            className="max-h-[min(24rem,var(--radix-dropdown-menu-content-available-height))] w-60"
          >
            <DropdownMenuRadioGroup
              value={parsed ? formatPlaygroundImageSize(width, height) : ""}
              onValueChange={onChange}
            >
              <DropdownMenuRadioItem value="">
                {t("playground.imageSizeAuto")}
              </DropdownMenuRadioItem>
              {SIZE_PRESET_GROUPS.map(({ label, sizes }) => (
                <DropdownMenuGroup key={label}>
                  <DropdownMenuSeparator />
                  <DropdownMenuLabel>{t(label)}</DropdownMenuLabel>
                  {sizes.map(({ width, height, ratio }) => {
                    const size = formatPlaygroundImageSize(width, height);
                    return (
                      <DropdownMenuRadioItem
                        key={size}
                        value={size}
                        className="gap-4"
                      >
                        <span className="flex-1 tabular-nums">
                          {width} × {height}
                        </span>
                        <span className="text-muted-foreground">{ratio}</span>
                      </DropdownMenuRadioItem>
                    );
                  })}
                </DropdownMenuGroup>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <ImageSizeFields width={width} height={height} onChange={onChange} />
      <p className="mt-4 text-xs text-muted-foreground">
        {t("playground.imageSizeRange", {
          min: PLAYGROUND_IMAGE_MIN_DIMENSION,
          max: PLAYGROUND_IMAGE_MAX_DIMENSION,
        })}
      </p>
      <Separator className="my-4" />
      <Field orientation="horizontal">
        <FieldLabel htmlFor="playground-image-quality">
          {t("playground.imageQuality")}
        </FieldLabel>
        <Select
          value={normalizePlaygroundImageQuality(quality)}
          onValueChange={onQualityChange}
        >
          <SelectTrigger id="playground-image-quality" className="h-8 w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent collisionPadding={16}>
            <SelectGroup>
              {PLAYGROUND_IMAGE_QUALITIES.map((value) => (
                <SelectItem key={value} value={value}>
                  {value}
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>
      </Field>
    </div>
  );
}
