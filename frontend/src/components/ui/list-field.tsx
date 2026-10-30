import * as React from "react";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";

type ListFieldProps = Pick<
  React.ComponentProps<"input">,
  "placeholder" | "disabled" | "className" | "aria-label" | "aria-labelledby" | "aria-describedby" | "aria-invalid"
> & {
  id: string;
  value: string[];
  onValueChange: (value: string[]) => void;
  /** `comma` renders a single-line input, `newline` a textarea. Defaults to `comma`. */
  separator?: "comma" | "newline";
};

/**
 * List input that keeps the typed text and splits, trims and dedupes it on blur
 * (frontend-design-system.spec.md DS75).
 */
export function ListField({
  id,
  value,
  onValueChange,
  separator = "comma",
  ...props
}: ListFieldProps) {
  const joiner = separator === "comma" ? ", " : "\n";
  const [text, setText] = React.useState(() => value.join(joiner));
  const [focused, setFocused] = React.useState(false);
  // The list `text` currently represents; a different list from the parent replaces the
  // text, but never while the user is typing.
  const [syncedValue, setSyncedValue] = React.useState(value);
  if (!focused && value !== syncedValue && value.join("\n") !== syncedValue.join("\n")) {
    setSyncedValue(value);
    setText(value.join(joiner));
  }

  const commit = () => {
    const next = [
      ...new Set(
        text
          .split(separator === "comma" ? /[,\n]/ : /\n/)
          .map((item) => item.trim())
          .filter(Boolean),
      ),
    ];
    setSyncedValue(next);
    setText(next.join(joiner));
    if (next.join("\n") !== value.join("\n")) onValueChange(next);
  };

  const shared = {
    id,
    value: text,
    autoComplete: "off",
    onFocus: () => setFocused(true),
    onBlur: () => {
      setFocused(false);
      commit();
    },
    ...props,
  };

  return separator === "comma" ? (
    <Input
      {...shared}
      onChange={(event) => setText(event.target.value)}
      onKeyDown={(event) => {
        // Enter submits the surrounding form without a blur, so commit first.
        if (event.key === "Enter") commit();
      }}
    />
  ) : (
    <Textarea {...shared} onChange={(event) => setText(event.target.value)} />
  );
}
