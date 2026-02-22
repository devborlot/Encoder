use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    pub produto: String,
    pub produtora: String,
    pub agencia: String,
    pub anunciante: String,
    pub diretor: String,
}

#[derive(Debug, Deserialize)]
struct CodesFileRaw {
    codes: HashMap<String, String>,
}

pub fn load_defaults(config_dir: &Path) -> Result<Defaults> {
    let path = config_dir.join("defaults.toml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Não foi possível ler {}", path.display()))?;
    let defaults: Defaults =
        toml::from_str(&content).with_context(|| format!("Erro ao parsear {}", path.display()))?;
    Ok(defaults)
}

pub fn load_codes(config_dir: &Path) -> Result<HashMap<u32, String>> {
    let path = config_dir.join("codes.toml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Não foi possível ler {}", path.display()))?;
    let raw: CodesFileRaw = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Erro ao parsear {}: {e}", path.display()))?;
    let codes = raw
        .codes
        .into_iter()
        .filter_map(|(k, v)| k.parse::<u32>().ok().map(|n| (n, v)))
        .collect();
    Ok(codes)
}

/// Extrai o código numérico do nome do arquivo.
/// Ex: "FEV_PROMO_17.mp4" → 17
pub fn extract_code_from_filename(filename: &str) -> Option<u32> {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);

    // Último segmento após '_'
    stem.rsplit('_').next().and_then(|s| s.parse::<u32>().ok())
}

/// Busca o registro na tabela de códigos.
/// Se o código > 50, subtrai 40 (ex: 60→20, 71→31, 72→32).
pub fn lookup_registro(code: u32, codes: &HashMap<u32, String>) -> Option<String> {
    if let Some(reg) = codes.get(&code) {
        return Some(reg.clone());
    }
    if code > 50 {
        return codes.get(&(code - 40)).cloned();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_code() {
        assert_eq!(extract_code_from_filename("FEV_PROMO_17.mp4"), Some(17));
        assert_eq!(extract_code_from_filename("FEV_PROMO_5.mp4"), Some(5));
        assert_eq!(extract_code_from_filename("VIDEO_123.mp4"), Some(123));
        assert_eq!(extract_code_from_filename("nocode.mp4"), None);
    }

    #[test]
    fn test_lookup_registro() {
        let mut codes = HashMap::new();
        codes.insert(20, "2024017422020-0".to_string());
        codes.insert(31, "2024017422031-6".to_string());

        assert_eq!(lookup_registro(20, &codes), Some("2024017422020-0".to_string()));
        assert_eq!(lookup_registro(60, &codes), Some("2024017422020-0".to_string()));
        assert_eq!(lookup_registro(71, &codes), Some("2024017422031-6".to_string()));
        assert_eq!(lookup_registro(99, &codes), None);
    }
}
