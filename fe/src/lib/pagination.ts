export function loadedPageLabel(
  pages: readonly (readonly unknown[])[],
  page: number,
  hasNextPage: boolean,
  noun: string,
): string {
  const count = pages[page]?.length ?? 0;
  if (count === 0) return `No ${noun} on this page`;

  const previous = pages
    .slice(0, page)
    .reduce((total, items) => total + items.length, 0);
  const end = previous + count;
  const total = hasNextPage
    ? ""
    : ` of ${pages.reduce((sum, items) => sum + items.length, 0)}`;
  return `Showing ${previous + 1}–${end}${total} ${noun}`;
}
