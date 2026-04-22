# Security

Keep tokens, OAuth callbacks, bearer credentials, device codes, and provider secrets out of repository files. Store operational secrets in the local environment mechanism used by the service manager.

Skills and packages must not contain secrets or instructions to bypass tool permissions. Replay remains limited to tools with explicit typed replay-safe contracts; memory, session, and project replay stay excluded until privacy classification is complete.

