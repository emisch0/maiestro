// A looping picture of a Windows 11 taskbar for the onboarding "Find the brain
// icon" step (#198): the pointer opens the `^` overflow, picks up the brain and
// drops it onto the taskbar. Pure CSS keyframes (styles.css, `.tray-demo`) on a
// fixed 300×140 stage, so it needs no animation library and no timers. With
// `done` it holds the final frame (brain on the taskbar); with reduced motion
// it shows one still frame with an arrow instead.

import LogoIcon from "../icons/logo.svg?react";

/** The overflow's other icons (flyout-relative cells); the brain fills the gap. */
const OTHER_APPS: [number, number][] = [[6, 6], [34, 6], [62, 6], [6, 34], [62, 34]];

export function TrayDragDemo({ done = false }: { done?: boolean }) {
  return (
    <div
      className={`tray-demo${done ? " done" : ""}`}
      role="img"
      aria-label={
        done
          ? "The brain icon on the taskbar."
          : "Click the ^ arrow on the taskbar, then drag the brain icon down onto the taskbar."
      }
    >
      <div className="tray-demo-flyout">
        {OTHER_APPS.map(([x, y]) => (
          <span key={`${x},${y}`} className="tray-demo-app" style={{ left: x + 3, top: y + 3 }} />
        ))}
      </div>

      <div className="tray-demo-taskbar">
        <span className="tray-demo-chevron">
          <svg viewBox="0 0 12 12" width="10" height="10">
            <path d="M2.5 7.5 6 4l3.5 3.5" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </span>
        <span className="tray-demo-sys">
          <svg viewBox="0 0 16 16" width="12" height="12">
            <path d="M1.5 6a9.5 9.5 0 0 1 13 0M4 8.7a6 6 0 0 1 8 0M6.5 11.3a2.5 2.5 0 0 1 3 0" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
          </svg>
          <svg viewBox="0 0 16 16" width="12" height="12">
            <path d="M2 6h3l4-3v10l-4-3H2z" fill="currentColor" />
            <path d="M11.5 5.5a3.5 3.5 0 0 1 0 5" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
          </svg>
        </span>
        <span className="tray-demo-clock">
          9:41
          <br />
          10/5
        </span>
      </div>

      <span className="tray-demo-drop" />
      <LogoIcon className="tray-demo-brain" aria-hidden="true" />

      <svg className="tray-demo-arrow" viewBox="0 0 60 60" width="60" height="60" aria-hidden="true">
        <path d="M10 8c18 2 28 14 30 34" fill="none" stroke="currentColor" strokeWidth="2" strokeDasharray="4 3" strokeLinecap="round" />
        <path d="M34 37l6 7 5-8" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
      </svg>

      <svg className="tray-demo-cursor" viewBox="0 0 14 20" width="14" height="20" aria-hidden="true">
        <path d="M1 1v15.5l4-3.6 2.6 5.8 2.6-1.1-2.6-5.7H13z" fill="#fff" stroke="#000" strokeWidth="1.1" strokeLinejoin="round" />
      </svg>
    </div>
  );
}
