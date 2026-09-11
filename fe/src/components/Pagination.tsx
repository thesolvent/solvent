import styles from "./Pagination.module.css";

export function Pagination({
  label,
  page,
  pageCount,
  disabled = false,
  onPage,
}: {
  label: string;
  page: number;
  pageCount: number;
  disabled?: boolean;
  onPage: (page: number) => void;
}) {
  return (
    <div className={styles.root}>
      <span className={styles.label}>{label}</span>
      <div className={styles.controls}>
        <button
          type="button"
          className={styles.step}
          disabled={disabled || page === 0}
          onClick={() => onPage(page - 1)}
        >
          ← Prev
        </button>
        {Array.from({ length: pageCount }, (_, index) => (
          <button
            key={index}
            type="button"
            className={index === page ? styles.numberActive : styles.number}
            disabled={disabled}
            aria-current={index === page ? "page" : undefined}
            onClick={() => onPage(index)}
          >
            {index + 1}
          </button>
        ))}
        <button
          type="button"
          className={styles.step}
          disabled={disabled || page >= pageCount - 1}
          onClick={() => onPage(page + 1)}
        >
          Next →
        </button>
      </div>
    </div>
  );
}
