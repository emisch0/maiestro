import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { PostSpawnConfirmDialog } from "./PostSpawnConfirmDialog";
import { OverlayDialog } from "./OverlayDialog";

const commands = ["pnpm install", "cargo fetch --manifest-path backend/Cargo.toml"];

function renderDialog() {
  const onChoose = vi.fn();
  const onClose = vi.fn();
  render(<PostSpawnConfirmDialog repo="acme/widget" commands={commands} onChoose={onChoose} onClose={onClose} />);
  return { onChoose, onClose };
}

describe("PostSpawnConfirmDialog (#225)", () => {
  it("lists every command in run order", () => {
    renderDialog();
    const items = screen.getByRole("list", { name: "Commands, in run order" }).querySelectorAll("li");
    expect([...items].map((li) => li.textContent)).toEqual(commands);
    expect(screen.getByRole("dialog")).toHaveTextContent("acme/widget");
  });

  it("starts on Allow this time when every command fits", () => {
    renderDialog();
    expect(screen.getByRole("button", { name: /Allow this time/ })).toHaveFocus();
  });

  it("sends one all-or-nothing choice per button", () => {
    const { onChoose } = renderDialog();
    screen.getByRole("button", { name: /Allow this time/ }).click();
    screen.getByRole("button", { name: /Always allow for this repo/ }).click();
    screen.getByRole("button", { name: /Always allow for all repos/ }).click();
    screen.getByRole("button", { name: /Ignore commands/ }).click();
    expect(onChoose.mock.calls.map((c) => c[0])).toEqual([
      { action: "run", allow_once: commands },
      { action: "allow_repo", commands },
      { action: "allow_global", commands },
      { action: "skip" },
    ]);
  });

  it("closes without choosing", () => {
    const { onChoose, onClose } = renderDialog();
    screen.getByRole("button", { name: "Close" }).click();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onChoose).not.toHaveBeenCalled();
  });

  it("Escape closes only the dialog on top, back to the preview below", () => {
    const closePreview = vi.fn();
    const onClose = vi.fn();
    render(
      <>
        <OverlayDialog title="Review & spawn" onClose={closePreview}>preview</OverlayDialog>
        <PostSpawnConfirmDialog repo="acme/widget" commands={commands} onChoose={vi.fn()} onClose={onClose} />
      </>,
    );
    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(closePreview).not.toHaveBeenCalled();
  });
});
