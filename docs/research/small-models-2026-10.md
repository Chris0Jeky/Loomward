# Small local models for the categoriser (research, 2026-10-26)

Research input for LW-140 (#879), not product authority. Primary sources were read on 2026-10-26 (Hugging Face model
cards and API, vendor docs, GitHub releases); nothing here was measured on the owner's machine. The owner allowed model
downloads to the slower disks (HUMAN_TODO q-49, `loomward-categoriser-model-downloads`). Models live outside Git under
`E:\loomward-models`, each with source, licence, size and SHA-256 in a local manifest.

## What "Ternary Bonsai 2" is

The owner named it as the kind of resource-efficient model worth testing. It is Prism ML's `Ternary-Bonsai-2` family
(2B, 4B, 9B, 27B; Apache-2.0): Qwen3.5 (and Qwen3.8 for 27B) with ternary weights at about 2 bits per weight.
Three packings exist: `Q2_0` loads in mainline llama.cpp from build b11555 (support merged 2026-10-11); `PQ2_0` and
`PTQ1_0` need Prism ML's llama.cpp fork. The vendor publishes quality figures only for the 27B, so quality for this task
is unknown until measured.

## Candidates (first wave, about 36.5 GB)

Byte sizes below are the recorded file sizes in the local `model-wave1.tsv` download plan, not runtime memory
measurements. Each packing is counted separately: **36,544,659,968 bytes = 36.544659968 decimal GB** across 25 files.

| Model | Packing: bytes | Why | Runtime notes |
|---|---|---|---|
| Qwen3.5-0.8B | Q4_K_M: 532,517,120; Q8_0: 811,843,840 | generative summariser candidate; quantisation sweep | llama.cpp / LM Studio |
| Qwen3.5-2B | Q4_K_M: 1,280,835,840; Q8_0: 2,012,012,800 | generative summariser candidate; quantisation sweep | llama.cpp / LM Studio |
| Qwen3.5-4B | Q4_K_M: 2,740,937,888 | generative summariser candidate | llama.cpp / LM Studio |
| Ternary Bonsai 2 2B | Q2_0: 546,689,504; PQ2_0: 517,312,992 | the owner's efficiency direction | Q2_0 needs llama.cpp >= b11555; PQ2_0 needs Prism ML's fork |
| Ternary Bonsai 2 4B | Q2_0: 1,212,103,520; PQ2_0: 1,146,465,120 | the owner's efficiency direction | same packing requirements as 2B |
| Ternary Bonsai 2 9B | Q2_0: 2,556,640,128; PQ2_0: 2,416,851,840 | the owner's efficiency direction | same packing requirements as 2B |
| Gemma 4 E2B | Q4_K_M: 3,106,738,272 | instruction model candidate | llama.cpp |
| Gemma 4 E4B | Q4_K_M: 4,977,171,584 | instruction model candidate | llama.cpp |
| LFM2.5 230M | Q4_K_M: 153,406,304; Q8_0: 246,598,496 | LFM Open License v1.0 (commercial use under $10M revenue) | llama.cpp |
| LFM2.5 1.2B Instruct | Q4_K_M: 730,895,168; Q8_0: 1,246,253,888 | same licence as 230M | llama.cpp |
| LFM2.5 2.6B | Q4_K_M: 1,674,455,040 | same licence as 230M | llama.cpp |
| Granite 4.2 3B | Q4_K_M: 2,244,011,552 | comparison baseline | llama.cpp |
| Phi-4-mini | Q4_K_M: 2,491,874,272 | comparison baseline | llama.cpp |
| SmolLM3-3B | Q4_K_M: 1,915,305,312 | comparison baseline | llama.cpp |
| Laya (421M, ModernBERT) | Q8_0: 449,397,600 | typed decisions only (no text generation); the untuned base scores 0.362 vs 0.766 fine-tuned on its own benchmark | llama-server `/v1/systemone`; the fine-tuned GGUF needs a separate runtime |
| BitNet b1.58 2B4T | i2_s: 1,187,801,280 | ternary CPU baseline | bitnet.cpp only |
| bge-small-en-v1.5 | Q8_0: 36,685,152 | embedding prototype (LW-024) | llama.cpp `--embeddings` |
| embeddinggemma-2 | Q8_0: 309,855,456 | embedding prototype (LW-024) | llama.cpp `--embeddings` |

The owner's baseline (a Qwen 27B-class model in LM Studio) is not downloaded; at Q4_K_M it exceeds 16 GB of VRAM.

## Runtime gap

LM Studio 0.4.26 pins llama.cpp b11337, older than what Bonsai 2 `Q2_0` (b11555) and Laya's endpoint need. A current
llama.cpp Windows Vulkan build (b11890, 2026-10-26) would cover them; downloading and running it is a separate owner
decision (it is an executable, not a model). Until then the screen runs on the models LM Studio's runtime can load.

## How the screen runs

`scripts/categoriser_synth.py` generates a synthetic corpus; `scripts/categoriser_bench.py` sends packets to a local
OpenAI-compatible endpoint (it refuses any non-local host) and reports per-axis accuracy and macro-F1, coverage,
selective accuracy, invalid answers, latency and, for summaries, unsupported claims. The promotion bar is docs/51
section 2.4; the ablation order is docs/51 section 4.8.

The synthetic corpus is a smoke screen, not a ranking. The summary checker reports a lexical lower bound on
unsupported claims, not semantic entailment: it checks numeric values and units, rejects spelled-out numbers,
checks extension candidates and selected marker phrases, and ties named newest/last-modified and oldest ages to
their respective content-write fields. Unrecognised wording can still escape it; a zero rate does not establish
faithfulness or meet the promotion bar.
