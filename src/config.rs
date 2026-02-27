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
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Deserialize)]
struct CodesFileRaw {
    codes: HashMap<String, String>,
}

/// Resolve o diretório de configuração efetivo.
/// Se `client` for informado, retorna `config_dir/client/`.
fn resolve_config_path(config_dir: &Path, client: Option<&str>) -> std::path::PathBuf {
    match client {
        Some(name) => config_dir.join(name),
        None => config_dir.to_path_buf(),
    }
}

pub fn load_defaults(config_dir: &Path) -> Result<Defaults> {
    load_defaults_for(config_dir, None)
}

pub fn load_defaults_for(config_dir: &Path, client: Option<&str>) -> Result<Defaults> {
    let dir = resolve_config_path(config_dir, client);
    let path = dir.join("defaults.toml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Não foi possível ler {}", path.display()))?;
    let defaults: Defaults =
        toml::from_str(&content).with_context(|| format!("Erro ao parsear {}", path.display()))?;
    Ok(defaults)
}

pub fn load_codes(config_dir: &Path) -> Result<HashMap<u32, String>> {
    load_codes_for(config_dir, None)
}

pub fn load_codes_for(config_dir: &Path, client: Option<&str>) -> Result<HashMap<u32, String>> {
    let dir = resolve_config_path(config_dir, client);
    let path = dir.join("codes.toml");
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

/// Lista subpastas de `config_dir` que contenham `defaults.toml` e `codes.toml`.
/// Retorna os nomes das subpastas (nomes dos clientes), ordenados alfabeticamente.
pub fn list_clients(config_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(config_dir) else {
        return Vec::new();
    };
    let mut clients: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter(|e| {
            let p = e.path();
            p.join("defaults.toml").exists() && p.join("codes.toml").exists()
        })
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    clients.sort();
    clients
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
