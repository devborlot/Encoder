//! Integração com a plataforma Peach (latam.peachvideo.com).
//!
//! Replica via REST o fluxo do portal web (engenharia reversa documentada
//! em `memory/peach-rest-flow.md`):
//!
//! 1. Login (`POST /login/login/login`) → cookies de sessão
//! 2. Init upload (`GET /amasv/app/modulos/subir/add_spot_upload_action.php`)
//!    → retorna credenciais AWS STS temporárias
//! 3. Upload S3 multipart direto pro bucket
//!
//! O Peach processa automaticamente quando o arquivo aparece no bucket.

pub mod auth;
pub mod config;
pub mod send;
pub mod status;
pub mod upload;

pub use auth::{PeachClient, SessionInfo};
pub use config::{DestinoEntry, PeachConfig, PeachCredentials, PeachDestinos};
pub use send::{SendRequest, ValidateResponse};
pub use status::SpotStatus;
pub use upload::{StsCredentials, UploadParams};

use anyhow::Result;
use std::path::Path;

/// Faz login + upload de um arquivo numa única chamada.
/// Útil pra wrapper de testes ou integração simples.
pub async fn login_and_upload<F>(
    credentials: &PeachCredentials,
    cfg: &PeachConfig,
    params: &UploadParams<'_>,
    on_progress: F,
) -> Result<String>
where
    F: Fn(u64, u64) + Send + Sync + 'static,
{
    let client = PeachClient::new()?;
    let session = client.login(&credentials.email, &credentials.password).await?;
    let sts = client.init_upload(params, cfg, &credentials.productora_id).await?;
    upload::s3_multipart_upload(params.video_path, &sts, on_progress).await?;
    Ok(format!(
        "Upload OK | id_envio={} | usuário={} ({})",
        sts.id_envio, session.nombre_usuario_activo, session.id_empresa
    ))
}

/// Helper: remove o traço do registro ANCINE.
/// Ex: "2024017422006-5" → "20240174220065"
pub fn registro_to_codigo(registro: &str) -> String {
    registro.replace('-', "")
}

/// Resolve o `codigo` (registro ANCINE sem traço) a partir do nome do arquivo
/// e da tabela de codes.toml carregada.
pub fn resolve_codigo_from_filename(
    filename: &str,
    codes: &std::collections::HashMap<u32, String>,
) -> Option<String> {
    let code = crate::config::extract_code_from_filename(filename)?;
    let registro = crate::config::lookup_registro(code, codes)?;
    Some(registro_to_codigo(&registro))
}

/// Localiza o `peach_credentials.toml` a partir do diretório de config.
pub fn credentials_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("peach_credentials.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registro_to_codigo() {
        assert_eq!(registro_to_codigo("2024017422006-5"), "20240174220065");
        assert_eq!(registro_to_codigo("2024017422024-3"), "20240174220243");
    }
}
