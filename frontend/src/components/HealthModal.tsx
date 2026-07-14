import { HealthCheck, HealthStatus } from "../api";
import { OverlayDialog } from "./OverlayDialog";

// The repo health-check modal state (#93): checks stream in one at a time, `total`
// (known once the first event lands) lets the modal stop the spinner, and
// `loading` stays true until the command resolves.
export type HealthState = {
  repo: string;
  checks: HealthCheck[];
  total: number | null;
  /** Title of the check currently running, shown beside the spinner. */
  running: string | null;
  loading: boolean;
  error: string | null;
};

const HEALTH_ICON: Record<HealthStatus, string> = {
  pass: "✓",
  fail: "✕",
  warn: "!",
  skipped: "–",
};

function HealthCheckRow({ check, nested }: { check: HealthCheck; nested?: boolean }) {
  return (
    <>
      <div className={`health-row health-row--${check.status}${nested ? " health-row--nested" : ""}`}>
        <span className={`health-icon health-icon--${check.status}`}>{HEALTH_ICON[check.status]}</span>
        <div className="health-row-text">
          <span className="health-label">{check.label}</span>
          {check.detail && <span className="health-detail">{check.detail}</span>}
          {check.command && <code className="health-command">{check.command}</code>}
        </div>
      </div>
      {check.sub.map((s) => (
        <HealthCheckRow key={s.id} check={s} nested />
      ))}
    </>
  );
}

export function HealthModal({ state, onRetry, onClose }: {
  state: HealthState;
  onRetry: () => void;
  onClose: () => void;
}) {
  // Spin while the run is in flight and more checks are still expected.
  const spinning = state.loading && (state.total === null || state.checks.length < state.total);
  return (
    <OverlayDialog
      title={`Health · ${state.repo}`}
      panelClass="health-dialog"
      onClose={onClose}
      footer={
        <div className="health-footer">
          <button className="btn-save" onClick={onClose}>Done</button>
        </div>
      }
    >
      <div className="health-body">
        {state.error ? (
          <div className="cleanup-confirm">
            <p className="cleanup-lead">Couldn't run health check</p>
            <pre className="tool-error-message">{state.error}</pre>
            <div className="issue-actions">
              <button className="btn-save" onClick={onRetry}>Retry</button>
            </div>
          </div>
        ) : (
          <>
            {state.checks.map((c) => <HealthCheckRow key={c.id} check={c} />)}
            {spinning && (
              <div className="health-progress">
                <span className="health-spinner" aria-hidden="true" />
                <span className="health-label">{state.running ?? "Running checks…"}</span>
                {state.total !== null && (
                  <span className="health-detail">{state.checks.length + 1}/{state.total}</span>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </OverlayDialog>
  );
}
