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

| Model | Why | Runtime notes |
|---|---|---|
| Qwen3.5-0.8B, 2B, 4B | small generative summarisers; Q4_K_M and Q8_0 for the quantisation sweep | llama.cpp / LM Studio |
| Ternary Bonsai 2 2B, 4B, 9B | the owner's efficiency direction | `Q2_0` needs llama.cpp >= b11555; LM Studio 0.4.26 bundles b11337 |
| Gemma 4 E2B, E4B | strong on-device instruction models | llama.cpp |
| LFM2.5 230M, 1.2B, 2.6B | very small, fast; LFM Open License v1.0 (commercial use under $10M revenue) | llama.cpp |
| Granite 4.2 3B, Phi-4-mini, SmolLM3-3B | comparison baselines | llama.cpp |
| Laya (421M, ModernBERT) | typed decisions only (no text generation); the untuned base scores 0.362 vs 0.766 fine-tuned on its own benchmark | llama-server `/v1/systemone`; the fine-tuned GGUF needs a separate runtime |
| BitNet b1.58 2B4T | ternary CPU baseline | bitnet.cpp only |
| bge-small-en-v1.5, embeddinggemma-2 | embedding prototypes (LW-024) | llama.cpp `--embeddings` |

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
