import { ReactNode, useEffect, useRef } from "react";

// The modal scaffold shared by the health, hide/snooze, and Start Work overlays:
// a click-to-dismiss scrim, a panel that stops propagation, and a header with a
// title + close button. Adds the dialog a11y the inline versions lacked —
// `role="dialog"`/`aria-modal`, Escape-to-close, and moving focus into the panel
// on open unless an inner control (e.g. an autoFocus input) already claimed it.
export function OverlayDialog({ title, panelClass, onClose, children, footer }: {
  title: string;
  panelClass?: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const panelRef = useRef<HTMLDivElement>(null);
  // Read onClose through a ref so the effect stays mount-only (callers pass a
  // fresh arrow each render).
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onCloseRef.current(); };
    document.addEventListener("keydown", onKey);
    const panel = panelRef.current;
    if (panel && !panel.contains(document.activeElement)) panel.focus();
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="overlay" onClick={onClose}>
      <div
        ref={panelRef}
        className={`overlay-panel${panelClass ? ` ${panelClass}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="overlay-header">
          <span className="overlay-title">{title}</span>
          <button className="icon-btn" onClick={onClose} aria-label="Close">✕</button>
        </div>
        {children}
        {footer}
      </div>
    </div>
  );
}
