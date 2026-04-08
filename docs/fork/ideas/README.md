# Ideas & Imported Research Notes

This directory stores **English summary notes** for useful research artifacts
that were found outside the canonical docs tree, primarily under `/tmp/`.

These notes exist so that good ideas do not disappear, while still preserving
the docs contract:

- English docs remain canonical
- roadmap/proposal material stays clearly separated from current-behavior docs
- imported research is marked as non-canonical until it is adopted into a phase

## Status

Everything in this directory should be treated as:

- research input
- idea backlog
- architectural context

and **not** as the current runtime contract.

## Imported Notes

| Note | Source artifacts | Best fit |
|------|------------------|----------|
| [memory-event-emission-research.md](memory-event-emission-research.md) | `/tmp/phase_e_report.md` | future observability / read-model slice |
| [self-learning-algorithms-notes.md](self-learning-algorithms-notes.md) | `/tmp/compass_artifact.md` | Phase 4.9 |
| [tools-extraction-analysis-notes.md](tools-extraction-analysis-notes.md) | `/tmp/tools_extraction_analysis.md` | Phase 4.8 tool-facts rollout |
| [memory-trace-and-legacy-architecture-notes.md](memory-trace-and-legacy-architecture-notes.md) | `/tmp/synapseclaw_memory_trace.md`, `/tmp/synapseclaw-phase43-memory-architecture.md`, `/tmp/synapseclaw_architecture.md` | historical memory/tool context |
| [orchestration-research-notes.md](orchestration-research-notes.md) | `/tmp/compass_artifact_wf-89b3b20e-d448-4366-b4ba-1ef14b0f3417_text_markdown.md` | future orchestration/pipeline work |
| [legacy-roadmap-notes.md](legacy-roadmap-notes.md) | `/tmp/synapseclaw-roadmap.md` | long-range idea backlog |

## Import Policy

When useful external notes appear again:

1. summarize them in English
2. state the original source file(s)
3. say what is still useful
4. say what is stale or non-canonical
5. link the note to the phase where it belongs
