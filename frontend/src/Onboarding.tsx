// One-time onboarding window (issue #98). A branded webview dialog — rather than a
// native alert, which can only show the generic OS icon — so it shows the mAIestro
// logo and name and can grow more options over time. The backend opens this window
// only on the very first run; pressing "Get started" (or closing the window, which
// accepts the defaults) records the choices and marks onboarding complete.
//
// To add an option: add a piece of state and an `OnboardingOption` row, then thread
// its value into `completeOnboarding` (and the backend `onboarding_complete` command).

import { useState } from "react";
import { api } from "./api";
import LogoIcon from "./icons/logo.svg?react";

export function Onboarding() {
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [busy, setBusy] = useState(false);

  const finish = () => {
    setBusy(true);
    // The command destroys this window; no need to handle the resolved promise.
    api.completeOnboarding(launchAtLogin).catch(() => setBusy(false));
  };

  return (
    <main className="onboarding">
      <div className="onboarding-brand">
        <LogoIcon className="onboarding-logo" aria-hidden="true" />
        <h1 className="onboarding-title">
          m<span className="ai">AI</span>estro
        </h1>
      </div>
      <p className="onboarding-subtitle">A few preferences to get you started.</p>

      <div className="onboarding-options">
        <OnboardingOption
          label="Launch at login"
          hint="Start mAIestro automatically when you log in."
          on={launchAtLogin}
          onToggle={() => setLaunchAtLogin((v) => !v)}
        />
      </div>

      <button className="onboarding-btn" disabled={busy} onClick={finish}>
        Get started
      </button>
      <p className="onboarding-foot">You can change these anytime in Settings › Preferences.</p>
    </main>
  );
}

/** One toggle-able onboarding preference: a label + hint on the left, an on/off
 *  switch (shared `.toggle-switch` styling with the Preferences panel) on the right. */
function OnboardingOption({
  label,
  hint,
  on,
  onToggle,
}: {
  label: string;
  hint: string;
  on: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="onboarding-option">
      <div className="toggle-text">
        <div className="onboarding-option-label">{label}</div>
        <div className="onboarding-option-hint">{hint}</div>
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={on}
        aria-label={label}
        className={`toggle-switch ${on ? "on" : ""}`}
        onClick={onToggle}
      >
        <span className="toggle-knob" />
      </button>
    </div>
  );
}
