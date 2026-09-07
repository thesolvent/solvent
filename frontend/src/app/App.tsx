import { Button } from "@/views/shared/ui/Button";

export function App() {
  return (
    <div className="flex min-h-full flex-col items-center justify-center gap-6 bg-lime">
      <div className="text-center">
        <h1 className="font-display text-7xl uppercase tracking-tight text-ink">
          Solvent
        </h1>
        <p className="mt-2 font-mono text-sm uppercase tracking-wide text-grey-700">
          frontend foundation
        </p>
      </div>
      <Button>Get started</Button>
    </div>
  );
}
