/** Transform chain layer, in execution order (urp-transform-system.spec.md PIPE-1). */
export type TransformLayer = "provider" | "global" | "apiKey";

/**
 * Scrolls the rule at `index` (position in the editor's full `value` array) of
 * the `layer` editor into the viewport center and focuses its configure button
 * (transform-config-ui.spec.md TCU-5c). Call it after the editor is visible.
 */
export function revealTransformRule(layer: TransformLayer, index: number): void {
  const row = document.querySelector<HTMLElement>(
    `[data-transform-layer="${layer}"] [data-transform-rule-index="${index}"]`
  );
  if (!row) {
    return;
  }
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  row.scrollIntoView({ block: "center", behavior: reducedMotion ? "auto" : "smooth" });
  row.querySelector<HTMLElement>("[data-transform-configure]")?.focus({ preventScroll: true });
}
