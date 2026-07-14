// The dismissible `cleanup-confirm` error block used across both windows: a lead
// line, the error text, and a Dismiss button. `variant` picks the body styling —
// a monospace `<pre>` (the default, for tool/command output) or a warnings list
// (the PR create/merge errors).
export function DismissibleError({ lead, message, onDismiss, variant = "pre" }: {
  lead: string;
  message: string;
  onDismiss: () => void;
  variant?: "pre" | "list";
}) {
  return (
    <div className="cleanup-confirm">
      <p className="cleanup-lead">{lead}</p>
      {variant === "list"
        ? <ul className="cleanup-warnings"><li>{message}</li></ul>
        : <pre className="tool-error-message">{message}</pre>}
      <div className="issue-actions">
        <button className="btn-ghost" onClick={onDismiss}>Dismiss</button>
      </div>
    </div>
  );
}
