import { useMemo, useState } from "react";
import { HideState } from "../api";
import { defaultSnoozeLocal, snoozePresets } from "../lib/snooze";
import { OverlayDialog } from "./OverlayDialog";

export function HideSnoozeDialog({ title, onConfirm, onClose }: {
  title: string;
  onConfirm: (hidden: HideState) => void;
  onClose: () => void;
}) {
  const [custom, setCustom] = useState(false);
  const [until, setUntil] = useState(defaultSnoozeLocal);
  const presets = useMemo(() => snoozePresets(Date.now()), []);

  const customMs = new Date(until).getTime();
  const customValid = !Number.isNaN(customMs);

  return (
    <OverlayDialog title={`Hide · ${title}`} panelClass="hide-dialog" onClose={onClose}>
      <div className="hide-dialog-body">
        <button className="hide-choice" onClick={() => onConfirm({ snooze_until: null })}>
          <span className="hide-choice-label">Hide</span>
          <span className="hide-choice-hint">Until you unhide it</span>
        </button>
        {presets.map((p) => (
          <button key={p.label} className="hide-choice" onClick={() => onConfirm({ snooze_until: p.at })}>
            <span className="hide-choice-label">{p.label}</span>
            <span className="hide-choice-hint">{p.hint}</span>
          </button>
        ))}
        {custom ? (
          <div className="hide-custom">
            <input
              className="text-input"
              type="datetime-local"
              value={until}
              autoFocus
              aria-label="Snooze until"
              onChange={(e) => setUntil(e.target.value)}
            />
            {!customValid && <p className="cred-error">Enter a valid date &amp; time.</p>}
            <div className="issue-actions">
              <button className="btn-save" disabled={!customValid} onClick={() => customValid && onConfirm({ snooze_until: customMs })}>Confirm</button>
              <button className="btn-ghost" onClick={() => setCustom(false)}>Back</button>
            </div>
          </div>
        ) : (
          <button className="hide-choice" onClick={() => setCustom(true)}>
            <span className="hide-choice-label">Custom…</span>
            <span className="hide-choice-hint">Pick a date &amp; time</span>
          </button>
        )}
      </div>
    </OverlayDialog>
  );
}
