This Linux amd64 build is based on the existing v7.3.2-custom10 fork (8fa2fc6c), not upstream v8. It retains the existing account storage, cooldown policy, model routing and retry settings.

Changes:
- For Claude-to-Claude and Gemini-to-Gemini requests through Antigravity, repair recognized thought/signature HTTP 400 errors once on the same credential. Gemini accumulated-session overflow gets one retry with a new request-local session ID; genuinely oversized requests still fail.
- Preserve explicit Claude tool choices instead of forcing VALIDATED.
- Request disabled thinking only where Antigravity model capabilities permit it; filter returned thought content while preserving text, tool calls and usage. Forward visible streaming chunks immediately and send keepalives while waiting. Models that require thinking may still have upstream latency.
- Apply the management memory setting to Go's soft memory limit. It is not an operating-system RSS cap and resets on restart to GOMEMLIMIT.
- Export isolated 403 account JSON before removing the unchanged exported files. Invalid files and failed response writes are retained. The server cannot confirm that a client saved the download; removal failures are logged.
- Count successful inference HTTP responses on both /v1 and /v1beta, release capacity for failures, and reserve pending capacity to avoid concurrent overshoot. Model listings and token counting do not consume this limit. The setting is process-local.

Validation: focused Antigravity, signature, management-limit and handler stream/error tests, plus Linux build. Existing unrelated baseline test failures in direct-Claude credential-wide cooldown expectations and OpenAI-compatible image tool results were reproduced before these changes; this is not a claim that the entire upstream suite passes. No live Google account or production load test was performed.

The archive contains the executable, example configuration, licenses and documentation. It contains no user configuration, account credentials or lock database. Replace only the executable during an upgrade. OAuth client build settings are injected from the repository's existing encrypted Actions secrets.
