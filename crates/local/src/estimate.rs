//! Rough memory estimates for running a model.
//!
//! memory ≈ weights + KV cache + overhead
//!
//! - **weights**: parameters × bytes per parameter (2 for bf16/fp16, about 1.05
//!   for 8-bit and 0.6 for 4-bit, since some layers stay in 16-bit);
//! - **KV cache**: 2 (keys and values) × layers × KV heads × head size ×
//!   2 bytes, per token of context;
//! - **overhead**: activations and framework memory, about 5% of the weights
//!   plus 0.4 GB.
//!
//! These are estimates. Real use depends on the framework version and on how
//! long the conversation gets.

use chummy_llm::Precision;
use serde::Serialize;

use crate::hardware::Hardware;

const GB: f64 = 1e9;

/// The model numbers an estimate needs.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct ModelShape {
    pub params: u64,
    pub layers: u64,
    pub kv_heads: u64,
    pub head_dim: u64,
    pub max_context: u64,
}

impl ModelShape {
    pub fn weight_bytes(&self, p: Precision) -> u64 {
        let per = match p {
            Precision::Half => 2.0,
            Precision::Int8 => 1.05,
            Precision::Int4 => 0.6,
        };
        (self.params as f64 * per) as u64
    }

    pub fn kv_bytes_per_token(&self) -> u64 {
        2 * self.layers * self.kv_heads * self.head_dim * 2
    }

    pub fn total_bytes(&self, p: Precision, context: u64) -> u64 {
        let w = self.weight_bytes(p);
        w + self.kv_bytes_per_token() * context + (w as f64 * 0.05) as u64 + (0.4 * GB) as u64
    }

    /// Context lengths worth showing: 4K, 16K and 32K, capped at the model's maximum.
    pub fn contexts(&self) -> Vec<u64> {
        let max = if self.max_context == 0 {
            32_768
        } else {
            self.max_context
        };
        let mut v: Vec<u64> = [4_096, 16_384, 32_768]
            .into_iter()
            .filter(|&c| c <= max)
            .collect();
        if v.is_empty() {
            v.push(max);
        }
        v
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Cell {
    pub precision: Precision,
    pub context: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupKind {
    NvidiaGpu,
    AppleSilicon,
    CpuOnly,
}

/// How a model fares on one kind of computer.
#[derive(Debug, Clone, Serialize)]
pub struct Fit {
    pub label: String,
    pub kind: SetupKind,
    /// Memory the model can actually use on this setup.
    pub usable_bytes: u64,
    /// Best option that fits: highest precision first, then longest context.
    pub best: Option<Cell>,
    pub this_computer: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Estimates {
    pub shape: ModelShape,
    /// Every precision × context combination.
    pub table: Vec<Cell>,
    pub fits: Vec<Fit>,
}

fn usable(kind: SetupKind, mem: u64) -> u64 {
    let m = mem as f64;
    (match kind {
        // CUDA context and display take some VRAM.
        SetupKind::NvidiaGpu => (m - 0.6 * GB).max(0.0),
        // macOS lets the GPU use roughly 70% of unified memory by default.
        SetupKind::AppleSilicon => m * 0.70,
        // Leave room for the OS and other apps.
        SetupKind::CpuOnly => m * 0.70,
    }) as u64
}

fn best_fit(shape: &ModelShape, kind: SetupKind, usable: u64) -> Option<Cell> {
    let precisions: &[Precision] = match kind {
        // bitsandbytes 8/4-bit loading is only offered on NVIDIA GPUs.
        SetupKind::NvidiaGpu => &[Precision::Half, Precision::Int8, Precision::Int4],
        _ => &[Precision::Half],
    };
    for &p in precisions {
        for &c in shape.contexts().iter().rev() {
            let bytes = shape.total_bytes(p, c);
            if bytes <= usable {
                return Some(Cell {
                    precision: p,
                    context: c,
                    bytes,
                });
            }
        }
    }
    None
}

pub fn estimate(shape: ModelShape, hw: &Hardware) -> Estimates {
    let mut table = Vec::new();
    for p in [Precision::Half, Precision::Int8, Precision::Int4] {
        for c in shape.contexts() {
            table.push(Cell {
                precision: p,
                context: c,
                bytes: shape.total_bytes(p, c),
            });
        }
    }

    let mut fits = Vec::new();
    let mut push = |label: String, kind: SetupKind, mem: u64, this: bool| {
        let u = usable(kind, mem);
        fits.push(Fit {
            label,
            kind,
            usable_bytes: u,
            best: best_fit(&shape, kind, u),
            this_computer: this,
        });
    };

    // This computer first.
    if hw.apple_silicon {
        push(hw.summary(), SetupKind::AppleSilicon, hw.ram_bytes, true);
    } else if let Some(g) = hw.nvidia.first() {
        push(hw.summary(), SetupKind::NvidiaGpu, g.vram_bytes, true);
    } else {
        push(hw.summary(), SetupKind::CpuOnly, hw.ram_bytes, true);
    }

    let g = |n: u64| n * 1_000_000_000;
    for gb in [8, 12, 16, 24, 48] {
        push(
            format!("NVIDIA GPU, {gb} GB"),
            SetupKind::NvidiaGpu,
            g(gb),
            false,
        );
    }
    for gb in [16, 32, 64] {
        push(
            format!("Mac with Apple Silicon, {gb} GB"),
            SetupKind::AppleSilicon,
            g(gb),
            false,
        );
    }
    push(
        "CPU only, 32 GB RAM (slow)".into(),
        SetupKind::CpuOnly,
        g(32),
        false,
    );

    Estimates { shape, table, fits }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Roughly Qwen2.5-7B.
    fn seven_b() -> ModelShape {
        ModelShape {
            params: 7_600_000_000,
            layers: 28,
            kv_heads: 4,
            head_dim: 128,
            max_context: 32_768,
        }
    }

    #[test]
    fn numbers_are_in_the_right_ballpark() {
        let s = seven_b();
        let half_gb = s.weight_bytes(Precision::Half) as f64 / GB;
        assert!((15.0..16.0).contains(&half_gb), "{half_gb}");
        // 28 layers × 4 heads × 128 × 2 × 2 bytes = 57 KB per token.
        assert_eq!(s.kv_bytes_per_token(), 57_344);
        let total_32k = s.total_bytes(Precision::Half, 32_768) as f64 / GB;
        assert!((17.0..19.0).contains(&total_32k), "{total_32k}");

        // (was short_context_models_still_get_a_row)
        {
            let s = ModelShape {
                max_context: 2048,
                ..seven_b()
            };
            assert_eq!(s.contexts(), vec![2048]);
        }
    }

    #[test]
    fn picks_quantization_only_on_nvidia() {
        let hw = Hardware {
            os: "linux",
            arch: "x86_64",
            ram_bytes: 16_000_000_000,
            nvidia: vec![],
            apple_silicon: false,
        };
        let e = estimate(seven_b(), &hw);
        let gpu12 = e
            .fits
            .iter()
            .find(|f| f.label == "NVIDIA GPU, 12 GB")
            .unwrap();
        assert_eq!(gpu12.best.as_ref().unwrap().precision, Precision::Int8);
        let gpu24 = e
            .fits
            .iter()
            .find(|f| f.label == "NVIDIA GPU, 24 GB")
            .unwrap();
        assert_eq!(
            gpu24.best.as_ref().map(|c| (c.precision, c.context)),
            Some((Precision::Half, 32_768))
        );
        let mac16 = e
            .fits
            .iter()
            .find(|f| f.label.contains("Mac") && f.label.contains("16"))
            .unwrap();
        assert!(mac16.best.is_none());
        assert!(e.fits[0].this_computer);
        assert_eq!(e.table.len(), 9);
    }
}
