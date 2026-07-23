// Spike: validate candle (pure-Rust local ML) for v2 chat before committing to
// the full build-out. Not part of the shipped app — standalone crate, throwaway
// once the viability call is made. See ARCHITECTURE.md §7 for the design this
// is testing.

use anyhow::{Error as E, Result};
use candle::quantized::gguf_file;
use candle::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2;
use std::time::Instant;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer};

// Representative chunks — stand-ins for real wiki content, sized/shaped like
// the actual sc-wiki-builder output (short paragraphs, product-specific terms).
const CHUNKS: &[(&str, &str)] = &[
    (
        "refrigerant-tracking.md",
        "Refrigerant Tracking lets subscribers log EPA-mandated refrigerant handling events \
         (leak checks, recharges, disposal) against HVAC/refrigeration assets. Feature flags \
         gating this: FF_REFRIGERANT_LOG (core logging UI), FF_EPA_COMPLIANCE_REPORT (regulatory \
         export), FF_REFRIGERANT_ASSET_LINK (ties log entries to Asset Management records).",
    ),
    (
        "asset-management.md",
        "Asset Management (labeled Assets/Equipment in nav) tracks physical equipment across a \
         subscriber's portfolio: HVAC units, refrigeration systems, kitchen equipment, fixtures. \
         Integrates with Work Order Management for repair history and with Refrigerant Tracking \
         for regulated equipment.",
    ),
    (
        "invoicing-and-payments.md",
        "Provider invoicing and payments covers proposal approval, invoice submission, payment \
         terms, and dispute resolution between subscribers and service providers on the platform.",
    ),
    (
        "work-order-management.md",
        "Work Order Management is the core lifecycle for dispatching, tracking, and closing repair \
         and maintenance work orders between subscribers and providers.",
    ),
    (
        "sc-mobile-subscriber.md",
        "The subscriber mobile app supports work order approval, technician check-in verification, \
         and photo capture for completed work.",
    ),
];

const QUERY: &str = "What Feature Flags are related to Refrigerant Tracking?";

fn main() -> Result<()> {
    let device = Device::Cpu; // worst-case realistic baseline: no Metal/CUDA assumed on collaborator machines

    println!("=== Stage 1: embedding model (all-MiniLM-L6-v2) ===");
    let t0 = Instant::now();
    let (bert, mut bert_tokenizer) = load_bert(&device)?;
    println!("bert load (incl. first-run download if needed): {:?}", t0.elapsed());

    let t0 = Instant::now();
    let mut texts: Vec<&str> = CHUNKS.iter().map(|(_, t)| *t).collect();
    texts.push(QUERY);
    let embeddings = embed_batch(&bert, &mut bert_tokenizer, &texts, &device)?;
    println!("embedded {} texts in {:?}", texts.len(), t0.elapsed());

    let query_emb = embeddings.get(embeddings.dim(0)? - 1)?;
    let mut scored: Vec<(f32, &str)> = vec![];
    for (i, (path, _)) in CHUNKS.iter().enumerate() {
        let chunk_emb = embeddings.get(i)?;
        let sim = cosine_sim(&query_emb, &chunk_emb)?;
        scored.push((sim, path));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("\nRetrieval ranking for: {QUERY:?}");
    for (score, path) in &scored {
        println!("  {score:.4}  {path}");
    }
    let top_chunk = CHUNKS.iter().find(|(p, _)| *p == scored[0].1).unwrap().1;

    println!("\n=== Stage 2: chat synthesis (Qwen2-0.5B-Instruct, Q4 GGUF) ===");
    let t0 = Instant::now();
    let (mut model, tokenizer) = load_qwen(&device)?;
    println!("qwen load (incl. first-run download if needed): {:?}", t0.elapsed());

    let prompt = format!(
        "<|im_start|>system\nAnswer using only the provided context. Be concise.<|im_end|>\n\
         <|im_start|>user\nContext:\n{top_chunk}\n\nQuestion: {QUERY}<|im_end|>\n<|im_start|>assistant\n"
    );
    run_chat(&mut model, &tokenizer, &prompt, &device)?;

    Ok(())
}

fn load_bert(device: &Device) -> Result<(BertModel, Tokenizer)> {
    // Spike note: downloaded via plain `curl` into /tmp/hf-cache instead of hf-hub's
    // built-in downloader — hf-hub's native-tls client fails cert validation against
    // this network's TLS-intercepting proxy even though system curl trusts it fine.
    // Not a candle/inference concern; the shipped app would need its own fix for this
    // (e.g. rustls with a bundled cert, or shelling out to curl) if collaborators sit
    // behind similar interception. See ARCHITECTURE.md §7 risk notes.
    let cache = std::path::Path::new("/tmp/hf-cache/minilm");
    let config_path = cache.join("config.json");
    let tokenizer_path = cache.join("tokenizer.json");
    let weights_path = cache.join("model.safetensors");

    let config: BertConfig = serde_json::from_str(&std::fs::read_to_string(config_path)?)?;
    let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(E::msg)?;
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, device)? };
    let model = BertModel::load(vb, &config)?;
    Ok((model, tokenizer))
}

fn embed_batch(
    model: &BertModel,
    tokenizer: &mut Tokenizer,
    texts: &[&str],
    device: &Device,
) -> Result<Tensor> {
    if let Some(pp) = tokenizer.get_padding_mut() {
        pp.strategy = PaddingStrategy::BatchLongest;
    } else {
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
    }
    let encodings = tokenizer.encode_batch(texts.to_vec(), true).map_err(E::msg)?;
    let token_ids: Vec<Tensor> = encodings
        .iter()
        .map(|e| Tensor::new(e.get_ids(), device).map_err(E::from))
        .collect::<Result<_>>()?;
    let attn_masks: Vec<Tensor> = encodings
        .iter()
        .map(|e| Tensor::new(e.get_attention_mask(), device).map_err(E::from))
        .collect::<Result<_>>()?;

    let token_ids = Tensor::stack(&token_ids, 0)?;
    let attn_mask = Tensor::stack(&attn_masks, 0)?;
    let token_type_ids = token_ids.zeros_like()?;

    let out = model.forward(&token_ids, &token_type_ids, Some(&attn_mask))?;

    // Mean pool over the attention mask, then L2 normalize — matches sentence-transformers.
    let mask_f = attn_mask.to_dtype(DTYPE)?.unsqueeze(2)?;
    let sum_mask = mask_f.sum(1)?;
    let summed = out.broadcast_mul(&mask_f)?.sum(1)?;
    let pooled = summed.broadcast_div(&sum_mask)?;
    let normalized = pooled.broadcast_div(&pooled.sqr()?.sum_keepdim(1)?.sqrt()?)?;
    Ok(normalized)
}

fn cosine_sim(a: &Tensor, b: &Tensor) -> Result<f32> {
    let dot = (a * b)?.sum_all()?.to_scalar::<f32>()?;
    let na = (a * a)?.sum_all()?.to_scalar::<f32>()?.sqrt();
    let nb = (b * b)?.sum_all()?.to_scalar::<f32>()?.sqrt();
    Ok(dot / (na * nb))
}

fn load_qwen(device: &Device) -> Result<(Qwen2, Tokenizer)> {
    let tokenizer =
        Tokenizer::from_file("/tmp/hf-cache/qwen-tok/tokenizer.json").map_err(E::msg)?;
    let model_path =
        std::path::PathBuf::from("/tmp/hf-cache/qwen-gguf/qwen2-0_5b-instruct-q4_0.gguf");

    let mut file = std::fs::File::open(&model_path)?;
    let content = gguf_file::Content::read(&mut file).map_err(|e| e.with_path(model_path))?;
    let model = Qwen2::from_gguf(content, &mut file, device)?;
    Ok((model, tokenizer))
}

fn run_chat(model: &mut Qwen2, tokenizer: &Tokenizer, prompt: &str, device: &Device) -> Result<()> {
    let tokens = tokenizer.encode(prompt, true).map_err(E::msg)?;
    let prompt_tokens = tokens.get_ids().to_vec();
    let sample_len = 200usize;

    let mut logits_processor = LogitsProcessor::from_sampling(299792458, Sampling::ArgMax);
    let mut all_tokens = vec![];

    let t0 = Instant::now();
    let input = Tensor::new(prompt_tokens.as_slice(), device)?.unsqueeze(0)?;
    let logits = model.forward(&input, 0)?;
    let logits = logits.squeeze(0)?;
    let mut next_token = logits_processor.sample(&logits)?;
    let prompt_dt = t0.elapsed();
    all_tokens.push(next_token);

    let eos_token = *tokenizer.get_vocab(true).get("<|im_end|>").unwrap();
    let t0 = Instant::now();
    let mut generated = 1;
    for index in 0..sample_len {
        if next_token == eos_token {
            break;
        }
        let input = Tensor::new(&[next_token], device)?.unsqueeze(0)?;
        let logits = model.forward(&input, prompt_tokens.len() + index)?;
        let logits = logits.squeeze(0)?;
        next_token = logits_processor.sample(&logits)?;
        all_tokens.push(next_token);
        generated += 1;
    }
    let gen_dt = t0.elapsed();

    let answer = tokenizer.decode(&all_tokens, true).map_err(E::msg)?;
    println!("\n--- answer ---\n{answer}\n--------------");
    println!(
        "prompt: {} tokens in {:?} ({:.1} tok/s)",
        prompt_tokens.len(),
        prompt_dt,
        prompt_tokens.len() as f64 / prompt_dt.as_secs_f64()
    );
    println!(
        "generation: {generated} tokens in {:?} ({:.1} tok/s)",
        gen_dt,
        generated as f64 / gen_dt.as_secs_f64()
    );
    Ok(())
}
