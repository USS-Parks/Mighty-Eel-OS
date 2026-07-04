import { Panel } from '../components/Panel';

/** Routing + spend dashboards. Wired to the AOG /v1/usage meter API in C3. */
export function RoutingView() {
  return (
    <div className="mx-auto max-w-6xl">
      <h1 className="mb-4 text-lg font-semibold text-ink">Routing &amp; Spend</h1>
      <Panel title="Routing and metering">
        <p className="text-sm text-muted">
          Live routing (local vs cloud, per provider), metering, and ROI / break-even
          from the gateway meter API land here in C3.
        </p>
      </Panel>
    </div>
  );
}
