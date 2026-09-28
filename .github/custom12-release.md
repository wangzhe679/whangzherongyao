This Linux amd64 build extends v7.3.2-custom11 with two Claude Opus 4.6 modes selected by the public model name.

| Model received by CPA | Upstream model | Response mode |
| --- | --- | --- |
| `claude-opus-4-6` or `claude-opus-4-6-thinking` | `claude-opus-4-6-thinking` | Existing answer-only behavior |
| `[思考]claude-opus-4-6` or `[思考]claude-opus-4-6-thinking` | `claude-opus-4-6-thinking` | Automatic thinking with upstream thinking content preserved |

The prefixed route takes precedence over disabled thinking or hidden-summary fields forwarded by a gateway. Both prefixed spellings are registered, so removing the `-thinking` suffix still selects the intended mode. Aliases inherit refreshed upstream capabilities and are removed when the upstream model is excluded. Text, tool calls, usage, and output budgets retain their existing handling.

No New API settings are changed. New API must allow the prefixed model name and preserve the `[思考]` prefix. This release does not resolve the separately reported 8192-token truncation.

Validation includes mock-upstream streaming and non-streaming tests for all four names, model registration and exclusion tests, payload-override tests, the focused Antigravity/signature regression suite, and a local server build. Release CI also checks management limits and handler stream/error behavior before building Linux amd64 with a GLIBC 2.17 baseline. No live Google account or production load test was performed.

The archive contains the executable, example configuration, licenses and documentation. It contains no user configuration, account credentials or lock database. Replace only the executable during an upgrade. OAuth client build settings are injected from the repository's existing encrypted Actions secrets.
