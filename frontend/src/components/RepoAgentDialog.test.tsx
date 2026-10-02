import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { RepoAgentDialog } from "./RepoAgentDialog";

describe("RepoAgentDialog (#214)", () => {
  it("checks a pinned repo's agent and keeps it unselectable", () => {
    const onConfirm = vi.fn();
    render(<RepoAgentDialog repo="acme/widget" current="codex" pinned onConfirm={onConfirm} onClose={() => {}} />);
    expect(screen.getByRole("dialog", { name: "Agentic Coding CLI · acme/widget" })).toBeInTheDocument();
    const codex = screen.getByRole("button", { name: /Codex CLI/ });
    expect(codex).toBeDisabled();
    expect(codex).toHaveAttribute("aria-current", "true");
    fireEvent.click(screen.getByRole("button", { name: /GitHub Copilot CLI/ }));
    expect(onConfirm).toHaveBeenCalledWith("copilot");
  });

  it("lets a repo on the global default pin that same agent, with no default option", () => {
    const onConfirm = vi.fn();
    render(<RepoAgentDialog repo="acme/widget" current="claude" pinned={false} onConfirm={onConfirm} onClose={() => {}} />);
    expect(screen.queryByText(/use global default/i)).not.toBeInTheDocument();
    const claude = screen.getByRole("button", { name: /Claude Code/ });
    expect(claude).toBeEnabled();
    expect(claude).toHaveTextContent("The global default now");
    fireEvent.click(claude);
    expect(onConfirm).toHaveBeenCalledWith("claude");
  });
});
