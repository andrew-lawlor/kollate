//! The models Kollate offers, and recognising their files when the user adds
//! them. Links are pinned to a revision so the checksums stay valid.

use std::path::Path;

/// What a file is for: the language model, or the vision projector that turns
/// an image into input for it. Every model needs both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Model,
    Vision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownFile {
    pub role: Role,
    /// The name it downloads as.
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub blake3: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownModel {
    /// Stable ID, stored with each transcription.
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub recommended: bool,
    pub files: [KnownFile; 2],
}

impl KnownModel {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn file(&self, role: Role) -> &KnownFile {
        self.files
            .iter()
            .find(|f| f.role == role)
            .expect("both roles")
    }
}

const MODELS: [KnownModel; 3] = [
    KnownModel {
        id: "qwen3-vl-4b",
        name: "Qwen3-VL 4B",
        summary: "Nearly the best at half the size. About 2 seconds a note without a graphics card.",
        recommended: true,
        files: [
            KnownFile {
                role: Role::Model,
                name: "Qwen3VL-4B-Instruct-Q4_K_M.gguf",
                url: "https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/1cd86afb9a95c410a6038ab3b40d8b578c892266/Qwen3VL-4B-Instruct-Q4_K_M.gguf?download=true",
                size: 2_497_281_664,
                blake3: "ca9ee88799c3c37eeb647faeeb1152814784af11bc0ed29e775fe1ae81523047",
            },
            KnownFile {
                role: Role::Vision,
                name: "mmproj-Qwen3VL-4B-Instruct-F16.gguf",
                url: "https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct-GGUF/resolve/1cd86afb9a95c410a6038ab3b40d8b578c892266/mmproj-Qwen3VL-4B-Instruct-F16.gguf?download=true",
                size: 836_180_256,
                blake3: "411e2c9f1a788322017b32f9e2a3f372bb8446cc6b4153e6ffe3ed1f19e0a2b0",
            },
        ],
    },
    KnownModel {
        id: "qwen3-vl-2b",
        name: "Qwen3-VL 2B",
        summary: "The smallest and fastest. Good on clear handwriting.",
        recommended: false,
        files: [
            KnownFile {
                role: Role::Model,
                name: "Qwen3VL-2B-Instruct-Q8_0.gguf",
                url: "https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/52d6c8ffea26cc873ac5ad116f8631268d7eb503/Qwen3VL-2B-Instruct-Q8_0.gguf?download=true",
                size: 1_834_427_424,
                blake3: "428bdc47b30a66b81ac4ea2edd4d377ab1202c4a5f67af04d08c1ab7e63b0c36",
            },
            KnownFile {
                role: Role::Vision,
                name: "mmproj-Qwen3VL-2B-Instruct-F16.gguf",
                url: "https://huggingface.co/Qwen/Qwen3-VL-2B-Instruct-GGUF/resolve/52d6c8ffea26cc873ac5ad116f8631268d7eb503/mmproj-Qwen3VL-2B-Instruct-F16.gguf?download=true",
                size: 819_394_848,
                blake3: "2a0cc2f06da1c977fd29973bb8b7d161e2c1475a21ca5e2da784cb9ada693ee1",
            },
        ],
    },
    KnownModel {
        id: "qwen3-vl-8b",
        name: "Qwen3-VL 8B",
        summary: "The most accurate. Best with a graphics card.",
        recommended: false,
        files: [
            KnownFile {
                role: Role::Model,
                name: "Qwen3VL-8B-Instruct-Q4_K_M.gguf",
                url: "https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct-GGUF/resolve/f982a07559d4a2f6c8744d840bf6fccab30eea96/Qwen3VL-8B-Instruct-Q4_K_M.gguf?download=true",
                size: 5_027_784_800,
                blake3: "d32dfd461e02fe3e6c6094b6c994ebaeda061b8470ab03eb2a691444a3d1aa22",
            },
            KnownFile {
                role: Role::Vision,
                name: "mmproj-Qwen3VL-8B-Instruct-F16.gguf",
                url: "https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct-GGUF/resolve/f982a07559d4a2f6c8744d840bf6fccab30eea96/mmproj-Qwen3VL-8B-Instruct-F16.gguf?download=true",
                size: 1_159_029_824,
                blake3: "1a784bbaa32b3768193edd3c9f86b5174881053eac0e93553691dbc7f17e3551",
            },
        ],
    },
];

/// The models Kollate offers, recommended first.
pub fn catalog() -> &'static [KnownModel] {
    &MODELS
}

/// Which known model and file `path` is, by size and then checksum.
/// `None` for anything else, including a damaged or partial download.
pub fn identify(path: &Path) -> std::io::Result<Option<(&'static KnownModel, &'static KnownFile)>> {
    let size = std::fs::metadata(path)?.len();
    let candidates: Vec<_> = MODELS
        .iter()
        .flat_map(|m| m.files.iter().map(move |f| (m, f)))
        .filter(|(_, f)| f.size == size)
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    let hash = hasher.finalize().to_hex();
    Ok(candidates
        .into_iter()
        .find(|(_, f)| f.blake3 == hash.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        assert_eq!(catalog().iter().filter(|m| m.recommended).count(), 1);
        assert!(catalog()[0].recommended);
        for m in catalog() {
            assert_eq!(m.file(Role::Model).role, Role::Model);
            assert_eq!(m.file(Role::Vision).role, Role::Vision);
            for f in &m.files {
                assert!(f.url.contains(f.name) && f.url.ends_with("?download=true"));
                assert_eq!(f.blake3.len(), 64);
            }
        }
        assert_eq!(catalog()[0].size(), 3_333_461_920);
    }

    #[test]
    fn identifies_only_exact_files() {
        let dir = std::env::temp_dir().join(format!("kollate-identify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = dir.join("other.gguf");
        std::fs::write(&other, b"not a model").unwrap();
        assert!(identify(&other).unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
