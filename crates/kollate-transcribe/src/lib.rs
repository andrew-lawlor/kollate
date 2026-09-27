//! Local handwriting transcription (SPEC §8a): a Qwen3-VL model run
//! in-process with llama.cpp, on the GPU through Vulkan when there is one and
//! on the CPU otherwise. Nothing leaves the computer, and nothing is
//! downloaded: the user adds the model files ([`catalog`], [`identify`]).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

use kollate_core::markup::{Reader, RgbImage};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::mtmd::{
    MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText, mtmd_default_marker,
};
use llama_cpp_2::sampling::LlamaSampler;

pub use catalog::{KnownFile, KnownModel, Role, catalog, identify};
pub use models::{Added, Installed, add, choose, installed, remove};

mod catalog;
mod models;

type Result<T> = kollate_core::Result<T>;

fn err(e: impl std::fmt::Display) -> kollate_core::Error {
    std::io::Error::other(e.to_string()).into()
}

const HANDWRITING: &str =
    "Transcribe the handwritten text in this image exactly. Output only the text.";
const PRINT: &str = "Transcribe the printed text in this image exactly. Output only the text.";
/// Latin script (Basic Latin through Latin Extended-B) and typographic
/// punctuation. Without it, small models sometimes answer in Cyrillic.
const LATIN: &str = r"root ::= [\x20-\x7E -ɏ‐-‧\n]+";
/// Longer than any margin note; stops a model that starts repeating itself.
const MAX_TOKENS: usize = 160;

/// llama.cpp's global state, set up once per process with its logging off.
fn backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    if let Some(b) = BACKEND.get() {
        return Ok(b);
    }
    llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default().with_logs_enabled(false));
    let backend = LlamaBackend::init().map_err(err)?;
    Ok(BACKEND.get_or_init(|| backend))
}

/// A loaded model. Loading takes a second or two; reading a note well under
/// a second on a GPU and a few seconds on a CPU.
pub struct Transcriber {
    model: LlamaModel,
    vision: MtmdContext,
    prompts: [String; 2],
}

impl Transcriber {
    /// Loads a model and its vision projector. With `gpu`, all layers go to
    /// the GPU if Vulkan finds one; otherwise everything runs on the CPU.
    pub fn load(model: &Path, vision: &Path, gpu: bool) -> Result<Self> {
        let backend = backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(if gpu { 999 } else { 0 });
        let model = LlamaModel::load_from_file(backend, model, &params).map_err(err)?;
        let vision_path = vision
            .to_str()
            .ok_or_else(|| err("model path isn't UTF-8"))?;
        let vision = MtmdContext::init_from_file(
            vision_path,
            &model,
            &MtmdContextParams {
                use_gpu: gpu,
                ..Default::default()
            },
        )
        .map_err(err)?;
        if !vision.support_vision() {
            return Err(err("this vision file can't read images"));
        }
        let template = model.chat_template(None).map_err(err)?;
        let prompt = |ask: &str| -> Result<String> {
            let message =
                LlamaChatMessage::new("user".into(), format!("{}\n{ask}", mtmd_default_marker()))
                    .map_err(err)?;
            model
                .apply_chat_template(&template, &[message], true)
                .map_err(err)
        };
        let prompts = [prompt(HANDWRITING)?, prompt(PRINT)?];
        Ok(Self {
            model,
            vision,
            prompts,
        })
    }

    fn read(&self, image: &RgbImage, prompt: &str) -> Result<String> {
        let backend = backend()?;
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(4096))
            .with_n_batch(2048);
        let ctx = self.model.new_context(backend, params).map_err(err)?;
        let bitmap =
            MtmdBitmap::from_image_data(image.width, image.height, &image.data).map_err(err)?;
        let chunks = self
            .vision
            .tokenize(
                MtmdInputText {
                    text: prompt.to_owned(),
                    add_special: false,
                    parse_special: true,
                },
                &[&bitmap],
            )
            .map_err(err)?;
        let mut n_past = chunks
            .eval_chunks(&self.vision, &ctx, 0, 0, 2048, true)
            .map_err(err)?;

        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::grammar(&self.model, LATIN, "root").map_err(err)?,
            LlamaSampler::greedy(),
        ]);
        let mut ctx = ctx;
        let mut batch = LlamaBatch::new(1, 1);
        let mut out = Vec::new();
        for _ in 0..MAX_TOKENS {
            let token = sampler.sample(&ctx, -1);
            if self.model.is_eog_token(token) {
                break;
            }
            sampler.accept(token);
            out.extend(
                self.model
                    .token_to_piece_bytes(token, 32, false, None)
                    .map_err(err)?,
            );
            batch.clear();
            batch.add(token, n_past, &[0], true).map_err(err)?;
            n_past += 1;
            ctx.decode(&mut batch).map_err(err)?;
        }
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    }
}

impl Reader for Transcriber {
    fn handwriting(&mut self, image: &RgbImage) -> Result<String> {
        self.read(image, &self.prompts[0].clone())
    }

    fn print(&mut self, image: &RgbImage) -> Result<String> {
        self.read(image, &self.prompts[1].clone())
    }
}
