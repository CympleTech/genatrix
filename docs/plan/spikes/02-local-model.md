# Spike 02 · An 8B model on a 16 GB Mac mini

**Question** (design doc 10): on this machine, can an 8B 4-bit model classify sensitivity and summarize messages fast enough and well enough?

**Verdict: PASS, with two conditions.** Summarization passes outright. Classification passes only when items are batched ten to a call; one item per call misses both the latency and the accuracy threshold. Rules must run before the model for the secret patterns, as design 02 already requires.

## Environment

- Mac mini, Apple M2 Pro, 16 GB, macOS 26.6.2
- Model: `mlx-community/Qwen3-8B-4bit`, 4.3 GB on disk, loaded through `crabllm-mlx` (in-process MLX via Swift FFI), release build
- Probe: a temporary Rust example inside the crabllm checkout; deleted after the run
- Data: 50 synthetic bilingual messages labelled by hand (17 public, 17 personal, 16 secret) and 5 long messages of 200 to 400 words. Synthetic labels stand in for the user's own judgement here; the real evaluation set (design 04) is built from the user's data once M1 has ingested some.

## Thresholds and results

| Check | Threshold | One item per call | Ten items per call |
|---|---|---|---|
| Classification agreement | ≥ 85 % on 50 | 78 % (39/50) | **92 % (46/50)** |
| Classification latency | ≤ 500 ms per item | 1 156 ms p50, 1 221 ms p95 | **356 ms per item** (3.2 to 4.0 s per batch) |
| Summary latency | ≤ 5 s per text | **3.06 s mean** (2.4 to 3.5 s) | |
| Summary usefulness | readable, factual | **yes**: three bullets, correct facts, correct language, no advice | |

Other measurements:

- Model load: 1.7 to 4.0 s cold.
- Fixed cost per call with a one-token reply to a one-word prompt: 242 ms, stable across five runs.
- Generation: 16 to 22 completion tokens per second.
- Memory: the model plus the process fit comfortably; no swapping observed with the IDE and browser open.

## What went wrong on the first run, and what it teaches

The first run scored 0/50. Qwen3 emits an empty `<think></think>` block even when thinking is switched off with the `/no_think` soft switch. With a 4-token budget the reply was the block itself, and the parser read the word "think". Fixing the parser to strip the block and allowing 16 tokens produced the numbers above.

Two implementation rules follow:

1. **Never trust a model to omit scaffolding.** The tool protocol layer (design 03) must strip reasoning blocks and validate the shape before the agent layer sees anything. Constrained decoding, when we have it, makes this moot for local models.
2. **Disable thinking at the template level**, not with a soft switch, when the runtime supports the `enable_thinking` template argument. The soft switch still costs tokens.

## Error analysis

Single-item misses: 10 of 11 were secret judged as personal. Batched misses: 2 of 4 were the same direction. Every under-escalation was a message whose secrecy is semantic rather than lexical: lab results, a divorce case, an offer letter, a mortgage, a therapy appointment, a WiFi password inside chatty text.

Messages with lexical markers (a six-digit verification code, a card number, "password:") were mostly caught, and every one of them is also a rule in design 02's default rule set. So in the real pipeline the model only sees what the rules could not decide, and the residual error rate is what matters.

Batching improved accuracy, not just throughput. Seeing ten messages side by side gives the model contrast; an isolated message has none. This is worth keeping even where throughput does not demand it.

## Impact on the design

- **Design 04, classifier role**: classification runs in batches of about ten during backfill and whenever more than one item is pending; a single live item may run alone and accept about 1.1 s. The 242 ms per-call floor makes single-item calls the wrong shape for bulk work.
- **Design 04, latency budget**: at 356 ms per item, a 100 000-message backfill spends about 10 hours on classification alone. Acceptable for a first import that runs overnight, and it argues again for rules first: every item a rule settles is an item the model never sees.
- **Design 02, rules**: the default rule set must cover one-time codes, card and account numbers, passwords, ID numbers, and known financial, medical, legal, and government sender domains. Semantic secrecy (health, legal, salary in prose) is the model's job and its weakest area; the user's overrides feed the evaluation set that measures it.
- **Design 03, tool protocol**: strip reasoning blocks; validate enum outputs; treat malformed output as a first-class result.
- **Design 04, model choice**: Qwen3-8B-4bit is a viable default for phase one. The selection task in Phase 0 compares it against one or two peers on the same set; the numbers here are its baseline.
- **Summaries** need no change. Output language followed input language without being asked for each text.

## Not measured here

- Extraction (commitments, dates) and drafting quality. They use the same model and will be measured against the user's own data in M3 and M4.
- Behaviour under memory pressure with the embedder loaded alongside. The embedder is small; this is checked in Phase 0.
- Tool calling on this model through `crabllm-mlx`. Needed for the planner role in M4, not before.
