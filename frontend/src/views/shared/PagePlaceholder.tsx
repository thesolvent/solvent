export function PagePlaceholder({
  title,
  note,
}: {
  title: string;
  note: string;
}) {
  return (
    <div className="mx-auto max-w-3xl px-6 py-24 text-center">
      <h1 className="font-display text-5xl text-ink uppercase">{title}</h1>
      <p className="mt-3 font-mono text-sm tracking-wide text-grey-500 uppercase">
        {note}
      </p>
    </div>
  );
}
