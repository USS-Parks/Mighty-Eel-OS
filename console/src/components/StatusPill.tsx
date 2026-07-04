import type { ReactNode } from 'react';

export type PillTone = 'ok' | 'warn' | 'bad' | 'signal' | 'neutral';

const TONES: Record<PillTone, string> = {
  ok: 'bg-ok/15 text-ok border-ok/30',
  warn: 'bg-warn/15 text-warn border-warn/30',
  bad: 'bg-bad/15 text-bad border-bad/30',
  signal: 'bg-signal/15 text-signal border-signal/30',
  neutral: 'bg-white/5 text-muted border-edge',
};

/** A small status chip in one of the semantic tones. */
export function StatusPill({
  tone = 'neutral',
  children,
}: {
  tone?: PillTone;
  children: ReactNode;
}) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium ${TONES[tone]}`}
    >
      {children}
    </span>
  );
}
