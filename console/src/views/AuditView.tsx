import { Panel } from '../components/Panel';

/** Unified audit search over the receipt ledger. Wired to /v1/receipts in C4. */
export function AuditView() {
  return (
    <div className="mx-auto max-w-6xl">
      <h1 className="mb-4 text-lg font-semibold text-ink">Audit</h1>
      <Panel title="Receipt ledger">
        <p className="text-sm text-muted">
          Search the unified ledger (WSF + AOG receipts) by correlation id, tenant,
          decision, or date lands here in C4.
        </p>
      </Panel>
    </div>
  );
}
