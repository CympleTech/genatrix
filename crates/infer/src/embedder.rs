//! The embedder: a small multilingual sentence model, in this process.
//!
//! Design: `docs/design/04-model-layer.md`, "角色" and "选型". The embedder
//! reads everything, so it runs only here, in the sandbox with no network,
//! beside the chat model. It is a BERT-shaped encoder loaded with `candle`
//! from a directory the core downloaded: `config.json`, `tokenizer.json`,
//! `model.safetensors`. Mean pooling over the attention mask, then a unit
//! norm, which is what the e5 family expects; the caller adds e5's
//! `query:` and `passage:` prefixes, because which one applies is the
//! caller's knowledge.
//!
//! On the CPU, on purpose: `candle`'s Metal backend lacks layer-norm for
//! this architecture. With Apple's Accelerate behind the matrix products a
//! 118M-parameter encoder reads about two thousand tokens a second here,
//! eight times what plain CPU code managed, and the GPU stays free for the
//! chat model.

use std::path::Path;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

/// The longest input the model sees, in tokens. Chunks are cut to fit well
/// under this; the truncation is the last line of defence.
const MAX_TOKENS: usize = 512;

/// A loaded embedding model.
pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    dims: usize,
}

impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder")
            .field("dims", &self.dims)
            .finish_non_exhaustive()
    }
}

impl Embedder {
    /// Load from a model directory.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let config: Config =
            serde_json::from_str(&std::fs::read_to_string(dir.join("config.json"))?)?;
        let device = Device::Cpu;
        let tensors = candle_core::safetensors::load(dir.join("model.safetensors"), &device)?;
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
        let model = BertModel::load(vb, &config)?;

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("tokenizer: {e}"))?;
        tokenizer
            .with_padding(Some(PaddingParams {
                strategy: PaddingStrategy::BatchLongest,
                ..PaddingParams::default()
            }))
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..TruncationParams::default()
            }))
            .map_err(|e| anyhow::anyhow!("tokenizer truncation: {e}"))?;

        Ok(Self {
            model,
            tokenizer,
            device,
            dims: config.hidden_size,
        })
    }

    /// Vector length.
    #[must_use]
    pub const fn dims(&self) -> usize {
        self.dims
    }

    /// One unit-norm vector per text, in order. Returns the vectors and the
    /// number of tokens read, for the usage line.
    pub fn embed(&self, texts: &[String]) -> anyhow::Result<(Vec<Vec<f32>>, usize)> {
        if texts.is_empty() {
            return Ok((Vec::new(), 0));
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenizing: {e}"))?;
        let tokens: usize = encodings
            .iter()
            .map(|e| e.get_attention_mask().iter().filter(|m| **m == 1).count())
            .sum();

        let ids: Vec<Vec<u32>> = encodings.iter().map(|e| e.get_ids().to_vec()).collect();
        let types: Vec<Vec<u32>> = encodings
            .iter()
            .map(|e| e.get_type_ids().to_vec())
            .collect();
        let masks: Vec<Vec<u32>> = encodings
            .iter()
            .map(|e| e.get_attention_mask().to_vec())
            .collect();
        let input_ids = Tensor::new(ids, &self.device)?;
        let token_type_ids = Tensor::new(types, &self.device)?;
        let attention_mask = Tensor::new(masks, &self.device)?;

        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;
        // Mean over the tokens that are really there.
        let mask = attention_mask.to_dtype(DType::F32)?.unsqueeze(2)?;
        let summed = hidden.broadcast_mul(&mask)?.sum(1)?;
        let counts = mask.sum(1)?.clamp(1e-9, f64::MAX)?;
        let mean = summed.broadcast_div(&counts)?;
        let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?;
        let unit = mean.broadcast_div(&norm)?;

        let vectors: Vec<Vec<f32>> = unit.to_vec2()?;
        Ok((vectors, tokens))
    }
}
