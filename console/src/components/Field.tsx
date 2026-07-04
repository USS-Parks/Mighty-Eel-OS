import type { ReactNode } from 'react';

/** A label → value row for summary panels. */
export function Field({
  label,
  children,
  mono = true,
}: {
  label: string;
  children: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-edge/50 py-2 last:border-0">
      <span className="label-dim shrink-0">{label}</span>
      <span className={`text-right text-sm text-ink ${mono ? 'font-mono' : ''}`}>
        {children}
      </span>
    </div>
  );
}
