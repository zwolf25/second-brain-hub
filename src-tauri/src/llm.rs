use crate::frontmatter;
use anyhow::{Context, Result};
use candle::quantized::gguf_file;
use candle::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer};
use walkdir::WalkDir;

const EMBED_REPO: &str = "sentence-transformers/all-MiniLM-L6-v2";
const CHAT_TOKENIZER_REPO: &str = "Qwen/Qwen2-0.5B-Instruct";
const CHAT_GGUF_REPO: &str = "Qwen/Qwen2-0.5B-Instruct-GGUF";
const CHAT_GGUF_FILE: &str = "qwen2-0_5b-instruct-q4_0.gguf";
const CHUNK_MAX_CHARS: usize = 2500; // ~500-800 tokens, per the chunking design in ARCHITECTURE.md §7

fn hf_url(repo: &str, filename: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{filename}")
}

/// `(model_label, downloaded_bytes, total_bytes)` — deliberately not tied to
/// Tauri's `AppHandle`/event system so this module can be tested (and reused)
/// without a running app; `lib.rs` wraps it with an `app.emit(...)` closure.
pub type ProgressFn<'a> = dyn Fn(&str, u64, u64) + 'a;

/// Streams a file to disk with periodic progress callbacks, skipping the
/// download entirely if it's already cached from a previous run.
///
/// Uses `reqwest` (native-tls), not `hf-hub`'s bundled `ureq` client — `ureq`'s
/// native-tls binding fails cert validation against this network's Zscaler TLS
/// interception even though the OS trust store (and reqwest's native-tls binding)
/// handles it fine. See ARCHITECTURE.md §7/§9.
fn download_file(on_progress: &ProgressFn, model_label: &str, url: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dest.parent().context("dest has no parent dir")?)?;
    let tmp = dest.with_extension("part");

    // Explicit timeouts — the bare `reqwest::blocking::get` free function has
    // none, so a real network hiccup would hang this (and the UI waiting on
    // it) forever with no error and no feedback.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(20 * 60))
        .build()
        .context("building HTTP client")?;
    let mut resp = client.get(url).send().context("download request")?;
    if !resp.status().is_success() {
        anyhow::bail!("download failed: HTTP {} for {url}", resp.status());
    }
    let total = resp.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(&tmp)?;
    let mut buf = [0u8; 65536];
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        if downloaded - last_emit > 2_000_000 || downloaded == total {
            last_emit = downloaded;
            on_progress(model_label, downloaded, total);
        }
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

pub struct EmbeddingPaths {
    config: PathBuf,
    tokenizer: PathBuf,
    weights: PathBuf,
}

fn embedding_paths(models_dir: &Path) -> EmbeddingPaths {
    let dir = models_dir.join("embedding");
    EmbeddingPaths {
        config: dir.join("config.json"),
        tokenizer: dir.join("tokenizer.json"),
        weights: dir.join("model.safetensors"),
    }
}

pub fn ensure_embedding_files(on_progress: &ProgressFn, models_dir: &Path) -> Result<EmbeddingPaths> {
    let p = embedding_paths(models_dir);
    download_file(on_progress, "embedding", &hf_url(EMBED_REPO, "config.json"), &p.config)?;
    download_file(on_progress, "embedding", &hf_url(EMBED_REPO, "tokenizer.json"), &p.tokenizer)?;
    download_file(on_progress, "embedding", &hf_url(EMBED_REPO, "model.safetensors"), &p.weights)?;
    Ok(p)
}

pub struct ChatPaths {
    tokenizer: PathBuf,
    gguf: PathBuf,
}

fn chat_paths(models_dir: &Path) -> ChatPaths {
    let dir = models_dir.join("chat");
    ChatPaths { tokenizer: dir.join("tokenizer.json"), gguf: dir.join(CHAT_GGUF_FILE) }
}

pub fn ensure_chat_files(on_progress: &ProgressFn, models_dir: &Path) -> Result<ChatPaths> {
    let p = chat_paths(models_dir);
    download_file(on_progress, "chat", &hf_url(CHAT_TOKENIZER_REPO, "tokenizer.json"), &p.tokenizer)?;
    download_file(on_progress, "chat", &hf_url(CHAT_GGUF_REPO, CHAT_GGUF_FILE), &p.gguf)?;
    Ok(p)
}

/// Kept loaded in app state for as long as the Chat tab is open — small (~90MB)
/// and fast enough (confirmed by the validation spike) that this isn't a
/// meaningful resource cost. Contrast with the chat model, which is loaded
/// fresh per-request and dropped (see `generate_answer`).
pub struct EmbeddingModel {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl EmbeddingModel {
    pub fn load(paths: &EmbeddingPaths) -> Result<Self> {
        let device = Device::Cpu;
        let config: BertConfig = serde_json::from_str(&std::fs::read_to_string(&paths.config)?)?;
        let mut tokenizer =
            Tokenizer::from_file(&paths.tokenizer).map_err(|e| anyhow::anyhow!(e))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[paths.weights.clone()], DTYPE, &device)?
        };
        let model = BertModel::load(vb, &config)?;
        Ok(Self { model, tokenizer, device })
    }

    /// Mean-pooled (over the attention mask), L2-normalized embeddings —
    /// matches the sentence-transformers pooling convention this model expects.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let encodings =
            self.tokenizer.encode_batch(texts.to_vec(), true).map_err(|e| anyhow::anyhow!(e))?;
        let token_ids: Vec<Tensor> = encodings
            .iter()
            .map(|e| Tensor::new(e.get_ids(), &self.device))
            .collect::<candle::Result<_>>()?;
        let attn_mask: Vec<Tensor> = encodings
            .iter()
            .map(|e| Tensor::new(e.get_attention_mask(), &self.device))
            .collect::<candle::Result<_>>()?;
        let token_ids = Tensor::stack(&token_ids, 0)?;
        let attn_mask = Tensor::stack(&attn_mask, 0)?;
        let token_type_ids = token_ids.zeros_like()?;

        let out = self.model.forward(&token_ids, &token_type_ids, Some(&attn_mask))?;
        let mask_f = attn_mask.to_dtype(DTYPE)?.unsqueeze(2)?;
        let sum_mask = mask_f.sum(1)?;
        let summed = out.broadcast_mul(&mask_f)?.sum(1)?;
        let pooled = summed.broadcast_div(&sum_mask)?;
        let normalized = pooled.broadcast_div(&pooled.sqr()?.sum_keepdim(1)?.sqrt()?)?;

        let n = normalized.dim(0)?;
        (0..n).map(|i| Ok(normalized.get(i)?.to_vec1::<f32>()?)).collect()
    }
}

#[derive(Clone)]
pub struct ChunkEmbedding {
    pub path: String,
    pub title: String,
    pub text: String,
    pub vector: Vec<f32>,
}

/// Splits body text on `## ` heading boundaries (hard-splitting anything still
/// oversized after that) — same "no markdown AST parser needed" reasoning as
/// title extraction in frontmatter.rs.
fn chunk_body(body: &str) -> Vec<String> {
    let mut chunks = vec![];
    let mut current = String::new();
    for line in body.lines() {
        if line.starts_with("## ") && !current.trim().is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
        if current.len() > CHUNK_MAX_CHARS {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Full rebuild, same "cheap enough at this scale" reasoning as index::rebuild —
/// walks the vault, chunks every doc, embeds everything in one batch call.
pub fn build_chunk_embeddings(vault_path: &Path, embed_model: &EmbeddingModel) -> Result<Vec<ChunkEmbedding>> {
    struct Pending {
        path: String,
        title: String,
        text: String,
    }
    let mut pending = vec![];

    for entry in WalkDir::new(vault_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let Some(ext) = path.extension() else { continue };
        if !ext.eq_ignore_ascii_case("md") {
            continue;
        }
        let filename = path.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled");
        if filename.starts_with("~$") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(path) else { continue };
        let parsed = frontmatter::parse(&raw, filename);

        for chunk_text in chunk_body(&parsed.body) {
            if chunk_text.trim().is_empty() {
                continue;
            }
            pending.push(Pending {
                path: path.to_string_lossy().to_string(),
                title: parsed.title.clone(),
                text: chunk_text,
            });
        }
    }

    let texts: Vec<&str> = pending.iter().map(|p| p.text.as_str()).collect();
    let vectors = embed_model.embed(&texts)?;

    Ok(pending
        .into_iter()
        .zip(vectors)
        .map(|(p, vector)| ChunkEmbedding { path: p.path, title: p.title, text: p.text, vector })
        .collect())
}

/// Vectors are already L2-normalized (see `EmbeddingModel::embed`), so a plain
/// dot product is the cosine similarity.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Brute-force top-k over the vault's chunk embeddings — sufficient at this
/// corpus size (a few hundred chunks), per ARCHITECTURE.md §7.
pub fn top_related(
    embed_model: &EmbeddingModel,
    chunks: &[ChunkEmbedding],
    query: &str,
    k: usize,
) -> Result<Vec<(f32, ChunkEmbedding)>> {
    let query_vec = embed_model.embed(&[query])?.into_iter().next().context("no embedding produced")?;
    let mut scored: Vec<(f32, &ChunkEmbedding)> =
        chunks.iter().map(|c| (dot(&query_vec, &c.vector), c)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(scored.into_iter().take(k).map(|(s, c)| (s, c.clone())).collect())
}

/// Loads the chat model fresh, generates one answer, then drops it — the
/// "opt-in synthesis, no standing background cost" design from ARCHITECTURE.md §7.
pub fn generate_answer(paths: &ChatPaths, question: &str, context_chunks: &[(f32, ChunkEmbedding)]) -> Result<String> {
    let device = Device::Cpu;
    let tokenizer = Tokenizer::from_file(&paths.tokenizer).map_err(|e| anyhow::anyhow!(e))?;

    let mut file = std::fs::File::open(&paths.gguf)?;
    let content = gguf_file::Content::read(&mut file).map_err(|e| e.with_path(paths.gguf.clone()))?;
    let mut model = Qwen2::from_gguf(content, &mut file, &device)?;

    let context: String = context_chunks
        .iter()
        .map(|(_, c)| format!("[{}]\n{}\n", c.title, c.text))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = format!(
        "<|im_start|>system\nAnswer using only the provided context. Quote identifiers (flag names, \
         codes) verbatim from the context — never invent or paraphrase one. If the context doesn't \
         contain an answer, say so plainly. Be concise. Cite doc titles in brackets.<|im_end|>\n\
         <|im_start|>user\nContext:\n{context}\n\nQuestion: {question}<|im_end|>\n<|im_start|>assistant\n"
    );

    let tokens = tokenizer.encode(prompt, true).map_err(|e| anyhow::anyhow!(e))?;
    let prompt_tokens = tokens.get_ids().to_vec();
    let mut logits_processor = LogitsProcessor::from_sampling(299792458, Sampling::ArgMax);
    let mut all_tokens = vec![];

    // Repeat penalty is what actually prevents the small model from looping on
    // the same list items — omitting it (an earlier oversight vs. the candle
    // reference example) is what produced the duplicated/hallucinated output
    // seen in the first real-vault test run.
    const REPEAT_PENALTY: f32 = 1.15;
    const REPEAT_LAST_N: usize = 64;

    let input = Tensor::new(prompt_tokens.as_slice(), &device)?.unsqueeze(0)?;
    let logits = model.forward(&input, 0)?.squeeze(0)?;
    let mut next_token = logits_processor.sample(&logits)?;
    all_tokens.push(next_token);

    let eos_token = *tokenizer
        .get_vocab(true)
        .get("<|im_end|>")
        .context("tokenizer vocab missing <|im_end|>")?;

    const MAX_NEW_TOKENS: usize = 300;
    for index in 0..MAX_NEW_TOKENS {
        if next_token == eos_token {
            break;
        }
        let input = Tensor::new(&[next_token], &device)?.unsqueeze(0)?;
        let logits = model.forward(&input, prompt_tokens.len() + index)?.squeeze(0)?;
        let start_at = all_tokens.len().saturating_sub(REPEAT_LAST_N);
        let logits = candle_transformers::utils::apply_repeat_penalty(
            &logits,
            REPEAT_PENALTY,
            &all_tokens[start_at..],
        )?;
        next_token = logits_processor.sample(&logits)?;
        all_tokens.push(next_token);
    }

    tokenizer.decode(&all_tokens, true).map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the real production path — download_file (the TLS fix),
    /// embedding + chunking + retrieval, and chat generation — against the
    /// actual shared vault. Machine-specific and network-dependent, so it's
    /// `#[ignore]`d by default: run explicitly with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn end_to_end_against_real_vault() {
        let vault = PathBuf::from(
            "/Users/zackwolf/Library/CloudStorage/OneDrive-Fortive/SVC-Product_Management - Second Brain/wikis",
        );
        assert!(vault.is_dir(), "real vault not found on this machine");

        let models_dir = std::env::temp_dir().join("sbs-llm-e2e-test-models");
        let noop_progress = |model: &str, downloaded: u64, total: u64| {
            eprintln!("  [{model}] {downloaded}/{total} bytes");
        };

        let embed_paths = ensure_embedding_files(&noop_progress, &models_dir).unwrap();
        let embed_model = EmbeddingModel::load(&embed_paths).unwrap();

        let chunks = build_chunk_embeddings(&vault, &embed_model).unwrap();
        assert!(!chunks.is_empty(), "expected chunks from the real vault");
        eprintln!("built {} chunk embeddings", chunks.len());

        // "refrigerant" alone (unlike the harder "what Feature Flags relate to
        // Refrigerant Tracking" phrasing) is well within this embedding model's
        // reach — see ARCHITECTURE.md §7/§9 for the documented retrieval gap on
        // technical/identifier-dense content and why this simpler query is the
        // right regression check here.
        let query = "refrigerant tracking compliance";
        let top = top_related(&embed_model, &chunks, query, 5).unwrap();
        assert!(!top.is_empty());
        for (score, c) in &top {
            eprintln!("  {score:.4}  {} ({} chars)", c.path, c.text.len());
        }
        assert!(
            top[0].1.text.to_lowercase().contains("refrigerant"),
            "expected the top result's content to actually mention refrigerant, got {}",
            top[0].1.path
        );

        let chat_paths = ensure_chat_files(&noop_progress, &models_dir).unwrap();
        let answer = generate_answer(&chat_paths, query, &top[..3]).unwrap();
        eprintln!("answer: {answer}");
        assert!(!answer.trim().is_empty());
    }
}
