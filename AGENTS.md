# JustQuery — project rules for AI agents

These rules govern how any AI agent works in this repository. They are part of the
project: keep this file updated when a new rule is agreed on.

1. **English only.** The whole project is written in English: code, comments,
   documentation, commit messages, and anything else that lands in the repo.
2. **Versioning.** Before 1.0.0, bump only the second (minor) version component;
   the third (patch) component is always `0` (`0.7.0` — yes; `0.7.1` — never).
3. **No automatic commits.** Never commit or push on your own initiative.
   Commit only after an explicit go-ahead from the user for that specific change.
4. **Single session.** Do all the work in the current session. Do not spawn
   additional agents or subagents unless the user explicitly asks for them.
5. **Discuss before editing.** Propose changes and talk through the sharp edges
   first; wait for an explicit go-ahead from the user before touching files.
