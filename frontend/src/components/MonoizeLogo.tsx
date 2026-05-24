import { cn } from "@/lib/utils";
import type { SVGProps } from "react";

interface MonoizeLogoProps extends SVGProps<SVGSVGElement> {
  className?: string;
}

/**
 * Monoize brand mark for in-app surfaces.
 *
 * The app mark omits the favicon's opaque dark plate. The M body uses
 * `currentColor`, so light and dark themes control contrast through the
 * surrounding text color while the beam colors preserve the brand identity.
 */
export function MonoizeLogo({ className, ...props }: MonoizeLogoProps) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 400 400"
      shapeRendering="geometricPrecision"
      className={cn("block", className)}
      aria-hidden="true"
      {...props}
    >
      <path
        d="M 88 120 L 118 88 L 198 165 L 282 88 L 310 120 L 310 310 L 256 310 L 256 178 L 199 226 L 177 212 Z"
        fill="currentColor"
      />
      <path
        d="M 88 289 L 177 212 L 177 270 L 126 310 L 88 310 Z"
        fill="currentColor"
      />
      <path d="M 88 120 L 88 168 L 177 212 Z" fill="#EE2E32" />
      <path d="M 88 182 L 88 224 L 177 212 Z" fill="#F6A51A" />
      <path d="M 88 238 L 88 289 L 177 212 Z" fill="#24B3CD" />
      <path d="M 224 226 L 256 202 L 256 250 Z" fill="#7FA8C4" />
    </svg>
  );
}
