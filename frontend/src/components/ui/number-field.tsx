import * as React from "react";
import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { FieldError } from "@/components/ui/field";

type NativeInputProps = Omit<
  React.ComponentProps<"input">,
  "id" | "value" | "defaultValue" | "onChange" | "type" | "min" | "max"
>;

export interface NumberFieldProps extends NativeInputProps {
  id: string;
  value: number | null;
  /** Receives only values that pass validation (or `null` when `allowEmpty`). */
  onValueChange: (value: number | null) => void;
  min?: number;
  max?: number;
  /** Defaults to true. */
  integer?: boolean;
  /** Defaults to false; an empty field is then a validation error. */
  allowEmpty?: boolean;
  /** Suffix text such as "ms", "USD" or a localized day unit. */
  unit?: string;
  onValidityChange?: (valid: boolean) => void;
}

const INTEGER_PATTERN = /^[+-]?\d+$/;
const DECIMAL_PATTERN = /^[+-]?(\d+\.?\d*|\.\d+)$/;

/**
 * Numeric input that keeps the typed text and validates on blur or Enter
 * (frontend-design-system.spec.md DS75).
 */
export const NumberField = React.forwardRef<HTMLInputElement, NumberFieldProps>(
  (
    {
      id,
      value,
      onValueChange,
      min,
      max,
      integer = true,
      allowEmpty = false,
      unit,
      onValidityChange,
      className,
      onBlur,
      onFocus,
      onKeyDown,
      "aria-describedby": ariaDescribedBy,
      ...props
    },
    ref,
  ) => {
    const { t } = useTranslation();
    const [text, setText] = React.useState(() => (value === null ? "" : String(value)));
    const [error, setError] = React.useState<string | null>(null);
    const [focused, setFocused] = React.useState(false);
    // The value `text` currently represents. A different `value` from the parent (reset,
    // discard, refetch) replaces the text, but never while the user is typing.
    const [syncedValue, setSyncedValue] = React.useState(value);
    if (!focused && value !== syncedValue) {
      setSyncedValue(value);
      setText(value === null ? "" : String(value));
      setError(null);
    }

    const reportedValidRef = React.useRef(true);
    const onValidityChangeRef = React.useRef(onValidityChange);
    React.useEffect(() => {
      onValidityChangeRef.current = onValidityChange;
    }, [onValidityChange]);

    React.useEffect(() => {
      const valid = error === null;
      if (reportedValidRef.current === valid) return;
      reportedValidRef.current = valid;
      onValidityChangeRef.current?.(valid);
    }, [error]);

    // A removed field must not keep its form blocked.
    React.useEffect(
      () => () => {
        if (!reportedValidRef.current) onValidityChangeRef.current?.(true);
      },
      [],
    );

    const rangeMessage = () => {
      const kind = integer ? "integer" : "number";
      if (min !== undefined && max !== undefined) {
        return t(`common.validation.${kind}Range`, { min, max });
      }
      if (min !== undefined) return t(`common.validation.${kind}Min`, { min });
      if (max !== undefined) return t(`common.validation.${kind}Max`, { max });
      return t(`common.validation.${kind}`);
    };

    const commit = () => {
      const trimmed = text.trim();
      let next: number | null = null;
      if (trimmed === "") {
        if (!allowEmpty) {
          setError(t("common.validation.required"));
          return;
        }
      } else {
        next = (integer ? INTEGER_PATTERN : DECIMAL_PATTERN).test(trimmed)
          ? Number(trimmed)
          : Number.NaN;
        if (
          !Number.isFinite(next) ||
          (integer && !Number.isSafeInteger(next)) ||
          (min !== undefined && next < min) ||
          (max !== undefined && next > max)
        ) {
          setError(rangeMessage());
          return;
        }
      }
      setError(null);
      setSyncedValue(next);
      if (next !== value) onValueChange(next);
    };

    const errorId = `${id}-error`;
    const describedBy = [ariaDescribedBy, error ? errorId : null].filter(Boolean).join(" ");
    const allowsNegative = min === undefined || min < 0;

    return (
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <Input
            ref={ref}
            id={id}
            type="text"
            inputMode={allowsNegative ? "text" : integer ? "numeric" : "decimal"}
            autoComplete="off"
            value={text}
            aria-invalid={error ? true : undefined}
            aria-describedby={describedBy || undefined}
            className={className}
            onChange={(event) => setText(event.target.value)}
            onFocus={(event) => {
              setFocused(true);
              onFocus?.(event);
            }}
            onBlur={(event) => {
              setFocused(false);
              commit();
              onBlur?.(event);
            }}
            onKeyDown={(event) => {
              // Enter submits the surrounding form without a blur, so commit first.
              if (event.key === "Enter") commit();
              onKeyDown?.(event);
            }}
            {...props}
          />
          {unit ? (
            <span className="shrink-0 text-sm text-muted-foreground">{unit}</span>
          ) : null}
        </div>
        {error ? <FieldError id={errorId}>{error}</FieldError> : null}
      </div>
    );
  },
);
NumberField.displayName = "NumberField";
