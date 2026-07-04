import { Panel } from '../components/Panel';

/** Overview + trust status. Populated with live trust-plane values in C2. */
export function OverviewView() {
  return (
    <div className="mx-auto max-w-6xl">
      <h1 className="mb-4 text-lg font-semibold text-ink">Overview</h1>
      <Panel title="Trust status">
        <p className="text-sm text-muted">
          Trust mode, bundle version, connectivity state, audit-chain integrity, and
          the tenant summary land here in C2.
        </p>
      </Panel>
    </div>
  );
}
