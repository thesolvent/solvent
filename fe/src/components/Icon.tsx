import {
  AlignHorizontalSpaceAround,
  Blend,
  Equal,
  MoreHorizontal,
  Search,
  SlidersHorizontal,
  Spline,
  type LucideIcon,
} from "lucide-react";

/**
 * The app's icons, named for what they mean here rather than for what they draw.
 *
 * Call sites ask for `search` or `curvePegged`, so swapping the underlying set — or overriding one
 * glyph the set draws badly at our sizes — is a change to this table alone.
 *
 * Curve glyphs stand for the three SwapVM shapes: a bezier for a curve priced across the whole
 * range, two edge bars for one priced inside a band, an equals for one held at a peg, and a blend for a
 * pool whose makers do not agree on one.
 */
const ICONS = {
  search: Search,
  settings: SlidersHorizontal,
  more: MoreHorizontal,
  curveXyc: Spline,
  curveConcentrated: AlignHorizontalSpaceAround,
  curvePegged: Equal,
  curveMixed: Blend,
} as const satisfies Record<string, LucideIcon>;

export type IconName = keyof typeof ICONS;

export function Icon({
  name,
  className,
}: {
  name: IconName;
  className?: string;
}) {
  const Glyph = ICONS[name];
  // Decorative: every icon sits inside a control that carries its own accessible name. The set
  // draws at a 24px grid with stroke 2, which reads heavy once scaled to our 15–20px boxes.
  return (
    <Glyph
      absoluteStrokeWidth
      aria-hidden="true"
      className={className}
      focusable="false"
      strokeWidth={1.75}
    />
  );
}
