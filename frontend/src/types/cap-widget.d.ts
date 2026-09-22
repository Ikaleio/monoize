import type { CapWidget } from "cap-widget";
import type { DetailedHTMLProps, HTMLAttributes } from "react";

declare module "react" {
  namespace JSX {
    interface IntrinsicElements {
      "cap-widget": DetailedHTMLProps<HTMLAttributes<CapWidget>, CapWidget> & {
        required?: boolean;
        "data-cap-api-endpoint"?: string;
        "data-cap-lang"?: string;
      };
    }
  }
}
