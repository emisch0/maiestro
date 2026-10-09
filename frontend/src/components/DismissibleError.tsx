import { useEffect, useState } from "react";
import { copyText } from "../lib/clipboard";

// The dismissible `cleanup-confirm` error block used across both windows: a lead
// line, the error text, and Copy / Dismiss buttons. `variant` picks the body
// styling — a monospace `<pre>` (the default, for tool/command output) or a
// warnings list (the PR create/merge errors). The text is selectable, and Copy
// puts the lead and the full message on the clipboard for a bug report.
export function DismissibleError({ lead, message, onDismiss, variant = "pre" }: {
  lead: string;
  message: string;
  onDismiss: () => void;
  variant?: "pre" | "list";
}) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);

  return (
    <div className="cleanup-confirm">
      <p className="cleanup-lead">{lead}</p>
      {variant === "list"
        ? <ul className="cleanup-warnings"><li>{message}</li></ul>
        : <pre className="tool-error-message">{message}</pre>}
      <div className="issue-actions">
        <button
          className="btn-ghost"
          title="Copy the error text"
          onClick={() => { void copyText(`${lead}\n${message}`).then(setCopied); }}
        >
          {copied ? "Copied" : "Copy"}
        </button>
        <button className="btn-ghost" onClick={onDismiss}>Dismiss</button>
      </div>
    </div>
  );
}
