import { Component, ErrorInfo, ReactNode } from "react";

// Guards the Settings detail panel so a render failure in one item (e.g. the
// JsonForms repo form) degrades to an inline message instead of unmounting the
// whole window and stranding the user with no sidebar to navigate back. Reset
// by keying it on the current selection, so picking another item remounts clean.
export class DetailErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Settings detail panel crashed:", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="cred-list">
          <div className="cleanup-confirm">
            <p className="cleanup-lead">Couldn't display this panel</p>
            <pre className="tool-error-message">{this.state.error.message || String(this.state.error)}</pre>
            <p className="cleanup-confirm-body" style={{ opacity: 0.7 }}>
              Pick another item on the left, or reopen Settings.
            </p>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}
