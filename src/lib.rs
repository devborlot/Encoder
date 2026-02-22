pub mod config;
pub mod encoder;
pub mod error;
pub mod metadata;
pub mod slate;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Procura o template da claquete em vários locais possíveis.
pub fn find_template(exe_dir: &Path) -> Result<PathBuf> {
    let candidates = [
        PathBuf::from("assets/template.png"),
        exe_dir.join("assets/template.png"),
        exe_dir.join("../assets/template.png"),
        // For target/release/ layout: go up two levels to project root
        exe_dir.join("../../assets/template.png"),
    ];

    for path in &candidates {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    bail!(
        "Template da claquete não encontrado. Coloque o arquivo em assets/template.png (1920x1080)"
    );
}
