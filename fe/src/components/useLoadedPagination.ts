import { useEffect, useState } from "react";

export function useLoadedPagination<T>(
  pages: readonly T[][],
  hasNextPage: boolean,
  loadNextPage: () => Promise<boolean>,
) {
  const [page, setPage] = useState(0);

  useEffect(() => {
    setPage((current) => Math.min(current, Math.max(0, pages.length - 1)));
  }, [pages.length]);

  async function select(nextPage: number) {
    if (nextPage < pages.length) {
      setPage(nextPage);
      return;
    }
    if (nextPage === pages.length && hasNextPage && (await loadNextPage())) {
      setPage(nextPage);
    }
  }

  return {
    items: pages[page] ?? [],
    page,
    pageCount: pages.length + Number(hasNextPage),
    select,
  };
}
