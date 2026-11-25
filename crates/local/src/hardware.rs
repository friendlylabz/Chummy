//! What this computer has to run models with.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vram_bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Hardware {
    pub os: &'static str,
    pub arch: &'static str,
    pub ram_bytes: u64,
    /// NVIDIA GPUs found with `nvidia-smi`.
    pub nvidia: Vec<Gpu>,
    /// Apple Silicon: GPU and CPU share `ram_bytes`.
    pub apple_silicon: bool,
}

impl Hardware {
    pub fn detect() -> Self {
        let ram_bytes = sysinfo::System::new()
            .map(|mut sys| {
                sys.refresh_memory();
                sys.total_memory()
            })
            .unwrap_or(0);
        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            ram_bytes,
            nvidia: nvidia_gpus(),
            apple_silicon: cfg!(all(target_os = "macos", target_arch = "aarch64")),
        }
    }

    /// One-line description, e.g. "NVIDIA RTX 4070 (12 GB), 32 GB RAM".
    pub fn summary(&self) -> String {
        let gb = |b: u64| (b as f64 / 1e9).round() as u64;
        if self.apple_silicon {
            return format!("Apple Silicon, {} GB unified memory", gb(self.ram_bytes));
        }
        match self.nvidia.first() {
            Some(g) => format!(
                "{} ({} GB), {} GB RAM",
                g.name,
                gb(g.vram_bytes),
                gb(self.ram_bytes)
            ),
            None => format!("No supported GPU found, {} GB RAM", gb(self.ram_bytes)),
        }
    }
}

fn nvidia_gpus() -> Vec<Gpu> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_nvidia_smi(&String::from_utf8_lossy(&o.stdout)),
        _ => vec![],
    }
}

pub(crate) fn parse_nvidia_smi(s: &str) -> Vec<Gpu> {
    s.lines()
        .filter_map(|l| {
            let (name, mib) = l.rsplit_once(',')?;
            let mib: u64 = mib.trim().parse().ok()?;
            Some(Gpu {
                name: name.trim().to_string(),
                vram_bytes: mib * 1024 * 1024,
            })
        })
        .collect()
}

/// Free space on the disk holding `path`, if it can be determined.
pub fn free_space(path: &Path) -> Option<u64> {
    let path = std::fs::canonicalize(path).ok()?;
    let disks = sysinfo::Disks::new_with_refreshed_list().ok()?;
    disks
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_csv() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 4070, 12282\nNVIDIA A100-SXM4-80GB, 81920\n");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(g[1].vram_bytes, 81920 * 1024 * 1024);
    }
}
