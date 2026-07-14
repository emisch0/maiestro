// The two hide/snooze affordances shared by the repo group and the work-item row.

// The pill shown on a hidden item — "Snoozed · 3h" or "Hidden" — that opens the
// item's command strip.
export function SnoozeLabel({ snoozeLabel, expanded, onClick }: {
  snoozeLabel: string;
  expanded: boolean;
  onClick: () => void;
}) {
  return (
    <button
      className="snooze-label"
      onClick={onClick}
      title="Manage visibility"
      aria-expanded={expanded}
    >
      {snoozeLabel ? `Snoozed · ${snoozeLabel}` : "Hidden"}
    </button>
  );
}

// The Hide…/Unhide command-strip button (which one shows depends on current
// hidden state).
export function HideCommandButton({ hidden, onHide, onUnhide }: {
  hidden: boolean;
  onHide: () => void;
  onUnhide: () => void;
}) {
  return hidden
    ? <button className="command-btn" onClick={onUnhide}>Unhide</button>
    : <button className="command-btn" onClick={onHide}>Hide…</button>;
}
