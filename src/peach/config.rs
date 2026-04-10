//! Configurações do Peach (credenciais user-level + IDs por cliente).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Credenciais do usuário Peach + ID da produtora (post.e).
/// Carregado de `config/peach_credentials.toml` (gitignored) ou env vars.
#[derive(Debug, Clone, Deserialize)]
pub struct PeachCredentials {
    pub email: String,
    pub password: String,
    /// ID da produtora no Peach (ex: "BRP190418" pra Post.E Motion)
    pub productora_id: String,
}

impl PeachCredentials {
    /// Carrega de env vars (`PEACH_EMAIL`, `PEACH_PASSWORD`, `PEACH_PRODUCTORA_ID`)
    /// ou do arquivo `config_dir/peach_credentials.toml`.
    pub fn load(config_dir: &Path) -> Result<Self> {
        // 1. Env vars (override)
        if let (Ok(email), Ok(password)) =
            (std::env::var("PEACH_EMAIL"), std::env::var("PEACH_PASSWORD"))
        {
            let productora_id = std::env::var("PEACH_PRODUCTORA_ID")
                .unwrap_or_else(|_| "BRP190418".to_string());
            return Ok(Self {
                email,
                password,
                productora_id,
            });
        }

        // 2. Arquivo
        let path = config_dir.join("peach_credentials.toml");
        if !path.exists() {
            bail!(
                "Credenciais do Peach não encontradas. Crie {} ou defina PEACH_EMAIL/PEACH_PASSWORD.",
                path.display()
            );
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Não foi possível ler {}", path.display()))?;
        let creds: Self = toml::from_str(&content)
            .with_context(|| format!("Erro ao parsear {}", path.display()))?;
        Ok(creds)
    }
}

/// Uma entrada de destino. Aceita tanto formato simples (só o ID como string)
/// quanto detalhado (`{id, nome}`).
///
/// Exemplos no TOML:
/// ```toml
/// hd = ["BR_GLOBO_112"]                                    # simples
/// hd = [{ id = "BR_GLOBO_112", nome = "TV Santa Cruz" }]   # detalhado
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DestinoEntry {
    Id(String),
    Detailed {
        id: String,
        #[serde(default)]
        nome: String,
    },
}

impl DestinoEntry {
    pub fn id(&self) -> &str {
        match self {
            DestinoEntry::Id(s) => s,
            DestinoEntry::Detailed { id, .. } => id,
        }
    }
    /// Nome amigável; se não houver, retorna o ID.
    pub fn label(&self) -> &str {
        match self {
            DestinoEntry::Id(s) => s,
            DestinoEntry::Detailed { id, nome } => {
                if nome.is_empty() {
                    id
                } else {
                    nome
                }
            }
        }
    }
}

/// Lista de destinos (emissoras) por codec. Faz parte do `[peach.destinos]`.
///
/// IDs vêm do portal Peach (`ID_EMPRESA`), ex: `BR_GLOBO_112` (TV Santa Cruz),
/// `BR_GLOBO_79` (TV Gazeta Vitória), `BR1230` (TV Vitória).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PeachDestinos {
    #[serde(default)]
    pub hd: Vec<DestinoEntry>,
    #[serde(default)]
    pub sd: Vec<DestinoEntry>,
}

impl PeachDestinos {
    pub fn is_empty(&self) -> bool {
        self.hd.is_empty() && self.sd.is_empty()
    }
    /// Retorna todos os IDs disponíveis (HD + SD), preservando ordem.
    pub fn all_ids(&self) -> Vec<String> {
        self.hd
            .iter()
            .chain(self.sd.iter())
            .map(|d| d.id().to_string())
            .collect()
    }
}

/// Configuração do Peach por cliente (tenant). Todos os campos do form que
/// não são extraídos do próprio VT (filename → pieza/código, encoding → fps/duração).
///
/// Faz parte do `defaults.toml` do cliente, no bloco `[peach]`.
#[derive(Debug, Clone, Deserialize)]
pub struct PeachConfig {
    // --- Anunciante / Marca / Produto ---
    /// ID do anunciante no Peach (ex: "BRA0743")
    pub avisador_id: String,
    /// CNPJ do anunciante (só dígitos)
    pub cnpj_avisador: String,
    /// ID da marca (numérico)
    pub id_marca: u32,
    /// ID do produto (numérico)
    pub id_producto: u32,
    #[serde(default)]
    pub avisador_extranjero: String, // "0" ou "1"

    // --- Agência ---
    /// ID da agência criativa (ex: "BR0741")
    pub agencia_id: String,
    /// CNPJ da agência (só dígitos)
    pub cnpj_agencia: String,
    #[serde(default)]
    pub agencia_extranjero: String, // "0" ou "1"

    // --- Campos com defaults sensatos ---
    #[serde(default = "default_formato")]
    pub formato: String, // "2" = MXF
    #[serde(default)]
    pub aspecto: String,
    #[serde(default = "default_pos_inicio")]
    pub pos_inicio: String,
    #[serde(default = "default_no")]
    pub vineta: String,
    #[serde(default = "default_no")]
    pub closed_caption: String,
    #[serde(default = "default_no")]
    pub tecla_sap: String,
    #[serde(default = "default_no")]
    pub lenguaje_senas: String,
    #[serde(default = "default_zero")]
    pub ad: String,
    #[serde(default = "default_zero")]
    pub surround: String,
    #[serde(default = "default_audio")]
    pub audio: String,
    #[serde(default = "default_tipo_crt")]
    pub tipo_crt: String,
    #[serde(default = "default_zero")]
    pub envio_exhibidor_bloqueado: String,
    #[serde(default = "default_zero")]
    pub elecciones: String,
    #[serde(default)]
    pub notificar_emails: String,
    #[serde(default)]
    pub campana: String,

    // --- Destinos para distribuição (envio para emissoras) ---
    #[serde(default)]
    pub destinos: PeachDestinos,
}

fn default_formato() -> String {
    "2".to_string()
}
fn default_pos_inicio() -> String {
    "07".to_string()
}
fn default_audio() -> String {
    "stereo".to_string()
}
fn default_tipo_crt() -> String {
    "A".to_string()
}
fn default_no() -> String {
    "no".to_string()
}
fn default_zero() -> String {
    "0".to_string()
}

impl PeachConfig {
    /// Defaults numéricos para os flags booleanos quando ausentes.
    pub fn avisador_extranjero_or_zero(&self) -> &str {
        if self.avisador_extranjero.is_empty() {
            "0"
        } else {
            &self.avisador_extranjero
        }
    }
    pub fn agencia_extranjero_or_zero(&self) -> &str {
        if self.agencia_extranjero.is_empty() {
            "0"
        } else {
            &self.agencia_extranjero
        }
    }
}

/// Bloco opcional dentro de `defaults.toml`. Quando presente, indica que
/// o cliente está configurado para envio via Peach.
#[derive(Debug, Clone, Deserialize)]
pub struct DefaultsWithPeach {
    #[serde(flatten)]
    pub base: crate::config::Defaults,
    pub peach: Option<PeachConfig>,
}

impl DefaultsWithPeach {
    /// Carrega `defaults.toml` (raiz ou cliente) com suporte ao bloco `[peach]`.
    pub fn load(config_dir: &Path, client: Option<&str>) -> Result<Self> {
        let dir = match client {
            Some(name) => config_dir.join(name),
            None => config_dir.to_path_buf(),
        };
        let path = dir.join("defaults.toml");
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Não foi possível ler {}", path.display()))?;
        let parsed: Self = toml::from_str(&content)
            .with_context(|| format!("Erro ao parsear {}", path.display()))?;
        Ok(parsed)
    }
}
