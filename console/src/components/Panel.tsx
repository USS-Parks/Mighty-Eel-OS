import type { ReactNode } from 'react';

/** The instrument-panel primitive: a bordered card with an optional titled head. */
export function Panel({
  title,
  actions,
  children,
  className,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  const hasHead = title !== undefined || actions !== undefined;
  return (
    <section className={`panel ${className ?? ''}`}>
      {hasHead ? (
        <header className="panel-head">
          <h2 className="text-sm font-semibold tracking-wide text-ink">{title}</h2>
          {actions ? <div className="flex items-center gap-2">{actions}</div> : null}
        </header>
      ) : null}
      <div className="p-4">{children}</div>
    </section>
  );
}
