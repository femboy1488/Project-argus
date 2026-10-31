import type { FitLevel, ModelEntry } from "./types";

// Same-family small models for speculative decoding (same tokenizer as the target).
export const DRAFTS = {
  qwen3: "unsloth/Qwen3-0.6B-GGUF:Q8_0",
  llama3: "bartowski/Llama-3.2-1B-Instruct-GGUF:Q4_K_M",
  gemma2: "bartowski/gemma-2-2b-it-GGUF:Q4_K_M",
} as const;
export const DRAFT_CHOICES: { value: string; label: string }[] = [
  { value: "auto", label: "Авто (по семейству модели)" },
  { value: DRAFTS.qwen3, label: "Qwen3 0.6B · для Qwen3" },
  { value: DRAFTS.llama3, label: "Llama 3.2 1B · для Llama 3.x / Hermes 3" },
  { value: DRAFTS.gemma2, label: "Gemma 2 2B · для Gemma 2 9B/27B" },
];

// Approximate Q4_K_M GGUF sizes. hfRepo is "<repo>:<quant>" for `llama-server -hf`.
// kvMbPer1k = 2 * layers * kv_heads * head_dim * ~1.06 B (q8_0) per token, x1000.
export const MODELS: ModelEntry[] = [
  { id: "gemma2-2b", name: "Gemma 2 2B", family: "Gemma", params: "2B", quant: "Q4_K_M", sizeGb: 1.7, kvMbPer1k: 55, score: 10,
    hfRepo: "bartowski/gemma-2-2b-it-GGUF:Q4_K_M", tags: ["fast", "chat"] },
  { id: "llama32-3b", name: "Llama 3.2 3B", family: "Llama", params: "3B", quant: "Q4_K_M", sizeGb: 2.0, kvMbPer1k: 60, score: 14,
    hfRepo: "bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M", tags: ["fast", "tools"], tools: true, draft: DRAFTS.llama3 },
  { id: "qwen25vl-3b", name: "Qwen2.5-VL 3B (зрение)", family: "Qwen", params: "3B", quant: "Q4_K_M", sizeGb: 2.9, kvMbPer1k: 20, score: 12,
    hfRepo: "ggml-org/Qwen2.5-VL-3B-Instruct-GGUF:Q4_K_M", tags: ["vision", "agent"], tools: true, vision: true,
    note: "для управления мышкой: видит скриншоты" },
  { id: "qwen3-4b", name: "Qwen3 4B Instruct 2507", family: "Qwen", params: "4B", quant: "Q4_K_M", sizeGb: 2.5, kvMbPer1k: 78, score: 20,
    hfRepo: "unsloth/Qwen3-4B-Instruct-2507-GGUF:Q4_K_M", tags: ["tools", "agent"], tools: true, draft: DRAFTS.qwen3 },
  { id: "mistral-7b", name: "Mistral 7B v0.3", family: "Mistral", params: "7B", quant: "Q4_K_M", sizeGb: 4.4, kvMbPer1k: 70, score: 16,
    hfRepo: "bartowski/Mistral-7B-Instruct-v0.3-GGUF:Q4_K_M", tags: ["chat"] },
  { id: "qwen25vl-7b", name: "Qwen2.5-VL 7B (зрение)", family: "Qwen", params: "7B", quant: "Q4_K_M", sizeGb: 5.7, kvMbPer1k: 31, score: 22,
    hfRepo: "ggml-org/Qwen2.5-VL-7B-Instruct-GGUF:Q4_K_M", tags: ["vision", "agent"], tools: true, vision: true,
    note: "для управления мышкой: видит скриншоты" },
  { id: "hermes3-8b", name: "Hermes 3 Llama 3.1 8B", family: "Nous", params: "8B", quant: "Q4_K_M", sizeGb: 4.9, kvMbPer1k: 70, score: 25,
    hfRepo: "NousResearch/Hermes-3-Llama-3.1-8B-GGUF:Q4_K_M", tags: ["tools", "agent"], tools: true, draft: DRAFTS.llama3 },
  { id: "llama31-8b", name: "Llama 3.1 8B", family: "Llama", params: "8B", quant: "Q4_K_M", sizeGb: 4.9, kvMbPer1k: 70, score: 24,
    hfRepo: "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF:Q4_K_M", tags: ["chat", "tools"], tools: true, draft: DRAFTS.llama3 },
  { id: "qwen3-8b", name: "Qwen3 8B", family: "Qwen", params: "8B", quant: "Q4_K_M", sizeGb: 5.0, kvMbPer1k: 78, score: 30,
    hfRepo: "unsloth/Qwen3-8B-GGUF:Q4_K_M", tags: ["tools", "agent"], tools: true, draft: DRAFTS.qwen3 },
  { id: "gemma2-9b", name: "Gemma 2 9B", family: "Gemma", params: "9B", quant: "Q4_K_M", sizeGb: 5.8, kvMbPer1k: 183, score: 27,
    hfRepo: "bartowski/gemma-2-9b-it-GGUF:Q4_K_M", tags: ["chat", "quality"], draft: DRAFTS.gemma2 },
  { id: "mistral-nemo-12b", name: "Mistral Nemo 12B", family: "Mistral", params: "12B", quant: "Q4_K_M", sizeGb: 7.5, kvMbPer1k: 87, score: 32,
    hfRepo: "bartowski/Mistral-Nemo-Instruct-2407-GGUF:Q4_K_M", tags: ["128k ctx", "tools"], tools: true },
  { id: "gemma2-27b", name: "Gemma 2 27B", family: "Gemma", params: "27B", quant: "Q4_K_M", sizeGb: 16.6, kvMbPer1k: 200, score: 40,
    hfRepo: "bartowski/gemma-2-27b-it-GGUF:Q4_K_M", tags: ["heavy"], draft: DRAFTS.gemma2 },
  { id: "qwen3-30b-a3b", name: "Qwen3 30B-A3B Instruct 2507", family: "Qwen", params: "30B MoE", quant: "Q4_K_M", sizeGb: 18.6, kvMbPer1k: 52, score: 55,
    hfRepo: "unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF:Q4_K_M", tags: ["MoE", "tools", "agent"], tools: true, moe: true, draft: DRAFTS.qwen3,
    note: "MoE: активны 3B параметров на токен — быстро для своего размера" },
  { id: "llama31-70b", name: "Llama 3.1 70B", family: "Llama", params: "70B", quant: "Q4_K_M", sizeGb: 42.5, kvMbPer1k: 174, score: 60,
    hfRepo: "bartowski/Meta-Llama-3.1-70B-Instruct-GGUF:Q4_K_M", tags: ["heavy"], draft: DRAFTS.llama3 },
];

/** Recommended for Agent mode (tool calling). */
export const AGENT_MODELS = ["qwen3-30b-a3b", "qwen3-8b", "hermes3-8b", "qwen25vl-7b"];

export const CTX_PRESETS = [
  { ctx: 8192, label: "8K", hint: "chat" },
  { ctx: 32768, label: "32K", hint: "long docs" },
  { ctx: 65536, label: "64K", hint: "agents · Hermes min" },
];

/** Weights (+8%) + q8_0 KV cache for ctx + ~400 MB compute buffers. */
export const requiredMb = (m: ModelEntry, ctx = 8192) =>
  Math.round(m.sizeGb * 1.08 * 1024 + m.kvMbPer1k * (ctx / 1000) + 400);

export function fitLevel(m: ModelEntry, usableMb: number, ctx = 8192): FitLevel {
  const need = requiredMb(m, ctx);
  if (need > usableMb) return "too-heavy";
  if (need > usableMb * 0.85) return "tight";
  return "fits";
}

/** Best curated model for the pool: comfortably fits, highest score; MoE gets a bonus (fast for its size). */
export function bestPick(usableMb: number, ctx = 8192, opts: { vision?: boolean } = {}): ModelEntry | undefined {
  const pool = MODELS.filter((m) => (opts.vision ? m.vision : !m.vision) && fitLevel(m, usableMb, ctx) === "fits");
  const rank = (m: ModelEntry) => (m.score ?? 0) + (m.moe ? 15 : 0) + (m.tools ? 2 : 0);
  return pool.sort((a, b) => rank(b) - rank(a))[0]
    ?? MODELS.filter((m) => fitLevel(m, usableMb, ctx) !== "too-heavy").sort((a, b) => rank(b) - rank(a))[0];
}

/** Draft model for speculative decoding, by family (unless the user picked one). */
export function draftFor(m: ModelEntry, choice: string): string | undefined {
  if (choice === "none") return undefined;
  if (choice !== "auto") return choice;
  if (m.draft) return m.draft;
  const n = `${m.hfRepo} ${m.name}`.toLowerCase();
  if (m.vision) return undefined;
  if (/qwen3/.test(n) && !/0\.6b/.test(n)) return DRAFTS.qwen3;
  if (/llama-?3\.[123]|hermes-?3/.test(n) && !/1b/.test(n)) return DRAFTS.llama3;
  if (/gemma-?2-(9|27)b/.test(n)) return DRAFTS.gemma2;
  return undefined;
}

// ---------------------------------------------------------------- Hugging Face helpers

/** "Qwen3-8B-Q4_K_M.gguf" -> "Q4_K_M"; also IQ4_XS, Q8_0, BF16, F16, UD-Q4_K_XL ... */
export function quantOf(file: string): string | undefined {
  const base = file.split("/").pop() ?? file;
  const m = base.match(/((?:UD-)?(?:I?Q\d+_[A-Z0-9_]+|Q\d+_\d|BF16|F16|F32|MXFP4(?:_MOE)?))(?:-\d{5}-of-\d{5})?\.gguf$/i);
  return m?.[1];
}

/** Rough KV estimate from parameter count when we don't know the architecture. */
export function kvGuess(paramsB: number): number {
  if (paramsB <= 2) return 40;
  if (paramsB <= 5) return 70;
  if (paramsB <= 10) return 80;
  if (paramsB <= 16) return 100;
  if (paramsB <= 35) return 160;
  return 200;
}

export function paramsFromName(name: string): number | undefined {
  const m = name.match(/(\d+(?:\.\d+)?)\s*[bB](?![a-z])/);
  return m ? parseFloat(m[1]) : undefined;
}
