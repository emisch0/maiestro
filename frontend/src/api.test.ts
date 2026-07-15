import { describe, it, expect } from "vitest";
import { mockIPC } from "@tauri-apps/api/mocks";
import { api } from "./api";

// The `api` object is the single seam between the frontend and the Rust backend
// (every call goes through Tauri `invoke`). These tests fake that boundary with
// `mockIPC`, so we can assert each wrapper forwards the right command name and
// argument shape, and returns the backend's value — without a running backend.
// `clearMocks` runs in the global afterEach (frontend/src/test/setup.ts).

describe("api seam", () => {
  it("forwards command name + args and returns the backend value", () => {
    const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: args as Record<string, unknown> });
      if (cmd === "path_exists") return true;
      return undefined;
    });

    return api.pathExists("~/src/widget/.env").then((ok) => {
      expect(ok).toBe(true);
      expect(calls).toEqual([
        { cmd: "path_exists", args: { path: "~/src/widget/.env" } },
      ]);
    });
  });

  it("maps snake_case wrapper params to the backend's argument keys", async () => {
    let seen: Record<string, unknown> | undefined;
    mockIPC((cmd, args) => {
      if (cmd === "repo_settings_get") {
        seen = args as Record<string, unknown>;
        return { repo: "acme/widget", env_files: [] };
      }
      return undefined;
    });

    const settings = await api.getRepoSettings("acme/widget");
    expect(seen).toEqual({ repo: "acme/widget" });
    expect(settings.repo).toBe("acme/widget");
  });

  it("propagates a backend error as a rejected promise", async () => {
    mockIPC((cmd) => {
      if (cmd === "repo_remove") throw new Error("boom");
      return undefined;
    });
    await expect(api.removeRepo("acme/widget")).rejects.toThrow("boom");
  });
});
