import { describe, it, expect } from "vitest";
import { repoGitHubUrl } from "./repos";

describe("repoGitHubUrl", () => {
  it("builds the homepage for an owner/name id", () => {
    expect(repoGitHubUrl("yanokamay-org/maiestro")).toBe("https://github.com/yanokamay-org/maiestro");
  });

  it("accepts dots, dashes and underscores", () => {
    expect(repoGitHubUrl("my_org/some.repo-name")).toBe("https://github.com/my_org/some.repo-name");
  });

  it.each(["", "maiestro", "a/b/c", "/name", "owner/", "own er/name", "owner/na me", "owner\name"])(
    "returns null for malformed id %j",
    (repo) => {
      expect(repoGitHubUrl(repo)).toBeNull();
    },
  );
});
