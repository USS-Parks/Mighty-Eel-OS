import type { PillTone } from './StatusPill';

export interface Bar {
  label: string;
  value: number;
  /** Formatted value shown at the row end (defaults to the raw number). */
  display?: string;
  tone?: PillTone;
}

const TONE_BG: Record<PillTone, string> = {
  ok: 'bg-ok',
  warn: 'bg-warn',
  bad: 'bg-bad',
  signal: 'bg-signal',
  neutral: 'bg-muted',
};

/** A dependency-free horizontal bar chart. Bar widths are proportional to the
 *  largest value; a small floor keeps non-zero bars visible. */
export function BarChart({ bars }: { bars: Bar[] }) {
  if (bars.length === 0) {
    return <p className="text-sm text-muted">No data yet.</p>;
  }
  const max = Math.max(1, ...bars.map((b) => b.value));
  return (
    <div className="space-y-2">
      {bars.map((b) => (
        <div key={b.label} className="flex items-center gap-3">
          <div className="w-24 shrink-0 truncate text-xs text-muted" title={b.label}>
            {b.label}
          </div>
          <div className="h-5 flex-1 overflow-hidden rounded bg-base">
            <div
              className={`h-full rounded ${TONE_BG[b.tone ?? 'signal']}`}
              style={{ width: `${b.value > 0 ? Math.max(2, (b.value / max) * 100) : 0}%` }}
            />
          </div>
          <div className="w-24 shrink-0 text-right font-mono text-xs text-ink">
            {b.display ?? String(b.value)}
          </div>
        </div>
      ))}
    </div>
  );
}
