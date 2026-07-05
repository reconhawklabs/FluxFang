// Shared placeholder for routes not yet built (Tasks 9.3+). Each stub page
// under `src/pages/` renders this with its own title; later tasks replace
// a stub file's contents with the real page, keeping the same default
// export + route path so `App.tsx`'s routing doesn't need to change.
export interface ComingSoonProps {
  title: string;
}

export default function ComingSoon({ title }: ComingSoonProps) {
  return (
    <div className="flex h-full flex-col items-start gap-2">
      <h1 className="text-xl font-semibold text-slate-100">{title}</h1>
      <p className="text-sm text-slate-500">This page is coming soon.</p>
    </div>
  );
}
