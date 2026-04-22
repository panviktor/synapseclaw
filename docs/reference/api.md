# API Reference

The gateway API backs the web UI and daemon-backed CLI flows. This page is a map of API groups rather than an exhaustive endpoint catalog.

Skills have the most complete API surface today and are documented in [skills-api.md](skills-api.md). Other endpoint groups should be treated as evolving until their user workflows stabilize.

Realtime call endpoints are documented in [realtime-calls.md](realtime-calls.md). Status and session-ledger reads are safe; start, answer, speak, and hangup require explicit confirmation before external calls are touched.

Voice runtime inspection is also available through `GET /api/voice/doctor`. This is a read-only preflight report for tty/headless status, audio runtime sockets, local playback and recording binaries, and lane readiness for `speech_synthesis` and `speech_transcription`.

CLI voice-mode state is available through `GET /api/voice/mode`, `POST /api/voice/mode`, and `DELETE /api/voice/mode`. These endpoints manage the same persisted `voice_mode:cli` profile used by `synapseclaw voice mode ...`, so web/operator controls do not need a separate storage path.
