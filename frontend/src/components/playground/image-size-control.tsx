import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
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
}

type Dimension = "width" | "height";

interface ImageSizeFieldsProps {
  width: number;
  height: number;
  onChange: (value: string) => void;
}

function ImageSizeFields({ width, height, onChange }: ImageSizeFieldsProps) {
  const { t } = useTranslation();
  const [widthInput, setWidthInput] = useState(String(width));
  const [heightInput, setHeightInput] = useState(String(height));

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

    if (dimension === "width") setWidthInput(String(nextValue));
    else setHeightInput(String(nextValue));
    updateSize(dimension, nextValue);
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
              value={inputValue}
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
          onValueChange={([nextValue]) => updateSize(dimension, nextValue)}
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

export function ImageSizeControl({ value, onChange }: ImageSizeControlProps) {
  const { t } = useTranslation();
  const parsed = parsePlaygroundImageSize(value);
  const width = parsed?.width ?? PLAYGROUND_IMAGE_DEFAULT_DIMENSION;
  const height = parsed?.height ?? PLAYGROUND_IMAGE_DEFAULT_DIMENSION;
  const sizeLabel = parsed
    ? `${parsed.width} × ${parsed.height}`
    : t("playground.imageSizeAuto");

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          aria-label={`${t("playground.imageSize")}: ${sizeLabel}`}
          className="h-8 max-w-[9rem] shrink-0 px-2 text-xs font-medium text-muted-foreground hover:text-foreground"
        >
          {sizeLabel}
        </Button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        sideOffset={8}
        align="start"
        collisionPadding={16}
        className="w-72"
      >
        <div className="mb-4 flex items-center justify-between gap-3">
          <p className="text-sm font-medium">{t("playground.imageSize")}</p>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => onChange("")}
            disabled={!parsed}
            className="h-7 px-2 text-xs"
          >
            {t("playground.imageSizeAuto")}
          </Button>
        </div>
        <ImageSizeFields
          key={value || "auto"}
          width={width}
          height={height}
          onChange={onChange}
        />
        <p className="mt-4 text-xs text-muted-foreground">
          {t("playground.imageSizeRange", {
            min: PLAYGROUND_IMAGE_MIN_DIMENSION,
            max: PLAYGROUND_IMAGE_MAX_DIMENSION,
          })}
        </p>
      </PopoverContent>
    </Popover>
  );
}
