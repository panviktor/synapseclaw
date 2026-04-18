# Tools Extraction Analysis Notes

Source artifact:

- `/tmp/tools_extraction_analysis.md`

## Why It Matters

Phase 4.8 now depends heavily on tools emitting explicit semantic facts.

That means tool prioritization matters. We should not convert tools at random.

This note is useful because it maps:

- which tools exist
- how large they are
- which ones are more standalone
- which ones have deeper core/runtime dependencies

## Most Useful Takeaways

- the tools surface is large and heterogeneous
- some groups are much better early candidates for explicit facts than others
- scheduling, messaging, memory, search, and file/runtime tools should be
  prioritized before niche integrations

## Practical Use

Use this note to prioritize the next batches of Phase 4.8 tool work:

1. tools with obvious structured results
2. tools that materially affect dialogue state and follow-up resolution
3. tools used in everyday flows

## Suggested Priority Buckets

### High Priority

- scheduling / cron / standing orders
- messaging / delivery
- memory search / recall / precedent lookup
- project/file context tools when they surface strong entities

### Medium Priority

- cloud / browser / http tools with structured result objects
- admin / configuration tools that can update durable defaults or environment facts

### Lower Priority

- niche third-party integrations
- tools whose result shape is mostly raw text and does not materially improve
  runtime resolution yet

## Best Fit

This note is most useful while finishing Phase 4.8 explicit tool-facts rollout.
