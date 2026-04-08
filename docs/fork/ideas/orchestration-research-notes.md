# Orchestration Research Notes

Source artifact:

- `/tmp/compass_artifact_wf-89b3b20e-d448-4366-b4ba-1ef14b0f3417_text_markdown.md`

## Why It Matters

This note is not primarily about memory, but it is useful because it captures
strong comparative lessons from:

- LangGraph
- AutoGen GraphFlow
- CrewAI
- MetaGPT
- OpenAI Agents SDK
- AWS Strands

## Most Useful Takeaways

- deterministic code-defined orchestration beats LLM-only flow control
- typed handoffs and bounded step contracts reduce hallucinated pipeline jumps
- tool interception / guardrails belong in the runtime, not in prompt hopes
- message filtering and scoped visibility are key for multi-agent sanity

## What To Keep

- graph/state-machine execution
- bounded tool scope per step/role
- interrupt / checkpoint patterns
- code-level pipeline contracts

## What To Treat Carefully

- this is broad comparative research, not a direct implementation plan
- it should inform future orchestration and pipeline phases, not memory slices

## Best Fit

This belongs to future orchestration/pipeline work, not to the main 4.8 memory
execution path.
