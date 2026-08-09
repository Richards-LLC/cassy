export function isMonospaceFamily(family: string): boolean {
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (!context) return true;
  context.font = `32px ${family}, monospace`;
  const widths = ["i", "M", "W", "0", "@", "#", ".", " "].map(
    (glyph) => context.measureText(glyph).width,
  );
  return widths.every((width) => Math.abs(width - widths[0]) < 0.01);
}
