---
name: release
description: Cut a signed, notarized mAIestro release — bump the version, build, and publish a GitHub Release with the .dmg attached. Runs the full scripts/release.sh pipeline from the primary checkout, usable from any session including a mAIestro-spawned worktree.
allowed-tools: Bash(git *) Bash(scripts/release.sh *) Bash(cd *) Read Write AskUserQuestion
---

You are cutting a release. The mechanics live in `scripts/release.sh` (three
idempotent phases: `bump`, `build`, `publish`); your job is the judgment around
them — where to run, what to bump, and the release notes — plus driving the
phases and reporting the result.

Releases are **local**: signing and notarization happen on the release Mac, not
CI. `build` is slow (a notarization round-trip to Apple). Because every phase is
idempotent, re-invoking this skill after a failure resumes rather than restarts.

## Procedure

1. **Find the primary checkout and work there.** A mAIestro-spawned session runs
   in a *feature* worktree that has no `.env.release` and must never be released
   from. Run `git worktree list`; the **first** entry is the primary checkout.
   Prefix every command below with `cd <primary-checkout> && …` (or verify you
   are already in it). Do **not** switch this session's branch.

2. **Confirm preconditions on the primary checkout.**
   ```
   git -C <primary> branch --show-current      # must be: main
   git -C <primary> status --porcelain          # must be empty
   git -C <primary> fetch origin && git -C <primary> pull --ff-only origin main
   ```
   If the branch isn't `main` or the tree is dirty, stop and tell the user.

3. **Determine the bump kind.** From the skill args (`/release minor`,
   `/release 1.2.0`) if given, else ask with AskUserQuestion among `patch`,
   `minor`, `major`. `patch` is the default for a routine release.

4. **Draft release notes.** Find the previous tag
   (`git -C <primary> describe --tags --abbrev=0` — none means this is the first
   release) and read the commits since it (`git -C <primary> log <prev>..HEAD
   --first-parent --pretty='%s'`, or the last 30 commits when there is no prior
   tag). Write brief, internal-facing markdown notes (this is a solo tool — no
   localization, no character limits). Group user-visible changes; drop pure
   chore/CI noise. **Show the draft and get approval** via AskUserQuestion
   ("Looks good, publish" vs "I'll tweak the wording") before touching anything.

5. **Bump and commit.**
   ```
   cd <primary> && scripts/release.sh bump <kind-or-version>
   git -C <primary> commit -am "chore(release): v<new-version>"
   git -C <primary> push origin main
   ```
   `bump` moves `tauri.conf.json`, `package.json`, `Cargo.toml`, and
   `Cargo.lock` together and asserts they agree; committing directly to `main`
   is the norm for this repo.

6. **Build** (slow — notarization):
   ```
   cd <primary> && scripts/release.sh build
   ```

7. **Publish.** Write the approved notes to a temp file, then:
   ```
   cd <primary> && scripts/release.sh publish --notes-file <tmp>
   ```
   This tags `v<version>`, creates the GitHub Release via the REST API, and
   uploads the notarized `.dmg`. It prints the release URL.

8. **Report** the version released and the GitHub Release URL. If any phase
   failed, say which one — re-running the skill resumes from there.

## Notes

- `bump` accepts `patch|minor|major` or an explicit `X.Y.Z`.
- `publish` requires `GITHUB_TOKEN` in `.env.release` (Contents: read/write on
  this repo) and refuses to publish an un-notarized `.dmg` or a dirty tree.
- Never release from a feature/spawned worktree — always the primary checkout.
- Run `scripts/release.sh` with no args to see the phase reference.
