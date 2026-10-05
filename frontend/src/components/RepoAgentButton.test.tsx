import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { RepoAgentButton } from "./RepoAgentButton";
import { AGENTS, AGENT_PRODUCTS } from "../lib/agents";

// The first path command of the rendered mark, enough to tell the logos apart.
const markPath = () => screen.getByRole("button").querySelector("svg path")?.getAttribute("d") ?? "";

describe("RepoAgentButton (#217)", () => {
  it("names each agent's product for a pinned repo", () => {
    for (const agent of AGENTS) {
      const { unmount } = render(<RepoAgentButton agent={agent} pinned onClick={() => {}} />);
      const btn = screen.getByRole("button", { name: `Agentic coding CLI: ${AGENT_PRODUCTS[agent]}` });
      expect(btn).toHaveAttribute("title", `Agentic coding CLI: ${AGENT_PRODUCTS[agent]}`);
      unmount();
    }
  });

  it("shows a different mark per agent", () => {
    const paths = AGENTS.map((agent) => {
      const { unmount } = render(<RepoAgentButton agent={agent} pinned onClick={() => {}} />);
      const d = markPath();
      unmount();
      return d;
    });
    expect(new Set(paths).size).toBe(AGENTS.length);
  });

  it("marks a repo following the global default", () => {
    render(<RepoAgentButton agent="codex" pinned={false} onClick={() => {}} />);
    expect(screen.getByRole("button")).toHaveAccessibleName("Agentic coding CLI: Codex CLI (global default)");
  });

  it("fires onClick", () => {
    const onClick = vi.fn();
    render(<RepoAgentButton agent="claude" pinned onClick={onClick} />);
    fireEvent.click(screen.getByRole("button"));
    expect(onClick).toHaveBeenCalledOnce();
  });
});
