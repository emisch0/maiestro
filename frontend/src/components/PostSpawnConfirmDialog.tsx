import { useCallback, useEffect, useRef, useState } from "react";
import { PostSpawnChoice } from "../api";
import { OverlayDialog } from "./OverlayDialog";

// Shown when the Spawn button is pressed and the repo has post-spawn commands
// not yet approved for it or for all repos (#225). Lists every command in run
// order; each choice covers all of them. Closing it cancels the spawn and leaves
// the Review & spawn preview as it was.
//
// Long commands wrap (continuation lines indented under the `$`) rather than
// scroll sideways, so the end of a command is never out of view. A long list
// scrolls; while commands are hidden below, a fade and a count say so, and
// keyboard focus starts on the list instead of "Allow this time", so Enter can't
// approve commands the user hasn't seen.
export function PostSpawnConfirmDialog({ repo, commands, onChoose, onClose }: {
  repo: string;
  commands: string[];
  onChoose: (choice: PostSpawnChoice) => void;
  onClose: () => void;
}) {
  const listRef = useRef<HTMLOListElement>(null);
  const allowRef = useRef<HTMLButtonElement>(null);
  // Commands still below the visible part of the list; null when nothing is hidden.
  const [hiddenBelow, setHiddenBelow] = useState<number | null>(null);

  const measure = useCallback(() => {
    const list = listRef.current;
    if (!list) return;
    const overflows = list.scrollHeight > list.clientHeight + 1;
    const atEnd = list.scrollTop + list.clientHeight >= list.scrollHeight - 2;
    if (!overflows || atEnd) {
      setHiddenBelow(null);
      return;
    }
    const bottom = list.getBoundingClientRect().bottom;
    setHiddenBelow([...list.children].filter((li) => li.getBoundingClientRect().top >= bottom - 4).length);
  }, []);

  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    measure();
    const overflows = list.scrollHeight > list.clientHeight + 1;
    (overflows ? list : allowRef.current)?.focus();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [measure]);

  const n = hiddenBelow ?? 0;
  return (
    <OverlayDialog title="Run setup commands?" panelClass="hide-dialog post-spawn-dialog" onClose={onClose}>
      <div className="hide-dialog-body">
        <p className="post-spawn-lead">This repo runs these commands in the new worktree before the editor opens:</p>
        <div className="post-spawn-term-wrap">
          <ol
            ref={listRef}
            className="post-spawn-term"
            aria-label="Commands, in run order"
            tabIndex={0}
            onScroll={measure}
          >
            {commands.map((cmd, i) => <li key={i}>{cmd}</li>)}
          </ol>
          {hiddenBelow !== null && <div className="post-spawn-term-fade" />}
        </div>
        {hiddenBelow !== null && (
          <p className="post-spawn-more">
            {n > 0
              ? `${n} more ${n === 1 ? "command" : "commands"} below. Scroll to see ${n === 1 ? "it" : "them"}.`
              : "Scroll to see the rest of the last command."}
          </p>
        )}
        <div className="post-spawn-choices">
          <button
            ref={allowRef}
            className="hide-choice hide-choice--primary"
            onClick={() => onChoose({ action: "run", allow_once: commands })}
          >
            <span className="hide-choice-label">Allow this time</span>
            <span className="hide-choice-hint">You&apos;ll be asked again on the next spawn</span>
          </button>
          <button className="hide-choice" onClick={() => onChoose({ action: "allow_repo", commands })}>
            <span className="hide-choice-label">Always allow for this repo</span>
            <span className="hide-choice-hint">Don&apos;t ask again in <b>{repo}</b></span>
          </button>
          <button className="hide-choice" onClick={() => onChoose({ action: "allow_global", commands })}>
            <span className="hide-choice-label">Always allow for all repos</span>
            <span className="hide-choice-hint">Don&apos;t ask again for these commands in any repo</span>
          </button>
          <div className="post-spawn-split" role="presentation" />
          <button className="hide-choice hide-choice--quiet" onClick={() => onChoose({ action: "skip" })}>
            <span className="hide-choice-label">Ignore commands</span>
            <span className="hide-choice-hint">Spawn the worktree without running them</span>
          </button>
        </div>
        <p className="session-hint">If a command changes, you&apos;ll be asked again. Esc goes back to the review.</p>
      </div>
    </OverlayDialog>
  );
}
