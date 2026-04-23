//! Upload de MP4 zipado pro Google Drive via Apps Script webhook.
//!
//! Fluxo:
//! 1. Lê o arquivo MP4 da disco
//! 2. Comprime em um ZIP em memória
//! 3. Codifica em base64
//! 4. Envia POST JSON pro webhook (Google Apps Script)
//! 5. Retorna a URL compartilhável retornada pelo script

use anyhow::{bail, Context, Result};
use base64::Engine;
use std::io::Write;
use std::path::Path;

/// Resultado do upload no Drive.
#[derive(Debug, Clone)]
pub struct DriveUploadResult {
    pub url: String,
    pub id: String,
    pub download_url: String,
}

/// Zipa um arquivo MP4 em memória e envia pro webhook do Google Apps Script.
/// Retorna a URL compartilhável do arquivo no Drive.
///
/// O webhook deve ter handler pra `action = "upload_mp4"` que decodifica
/// o base64, cria o arquivo no Drive na `folder_id` e retorna JSON:
/// `{status, url, id, download_url}`.
pub async fn upload_mp4_zipped(
    webhook_url: &str,
    mp4_path: &Path,
    folder_id: &str,
) -> Result<DriveUploadResult> {
    if webhook_url.is_empty() {
        bail!("webhook_url vazio");
    }
    if !mp4_path.exists() {
        bail!("Arquivo não existe: {}", mp4_path.display());
    }

    crate::log::emit(format!("[drive] Zipando {}", mp4_path.display()));
    let zip_bytes = zip_file(mp4_path).context("falha ao zipar MP4")?;
    crate::log::emit(format!(
        "[drive] Zip pronto: {} bytes (original {} bytes)",
        zip_bytes.len(),
        std::fs::metadata(mp4_path).map(|m| m.len()).unwrap_or(0)
    ));

    let b64 = base64::engine::general_purpose::STANDARD.encode(&zip_bytes);
    let filename = format!(
        "{}.zip",
        mp4_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("arquivo")
    );

    let payload = serde_json::json!({
        "action": "upload_mp4",
        "filename": filename,
        "content_base64": b64,
        "mime_type": "application/zip",
        "folder_id": folder_id,
    });

    crate::log::emit(format!(
        "[drive] Enviando {filename} ({} bytes base64) pro webhook...",
        b64.len()
    ));

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(300)) // 5 min pra uploads grandes
        .build()
        .context("falha ao criar HTTP client")?;

    let res = client
        .post(webhook_url)
        .json(&payload)
        .send()
        .await
        .context("falha no POST webhook drive")?;

    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("webhook drive retornou {}: {}", status, &body[..body.len().min(500)]);
    }

    let parsed: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("falha ao parsear resposta do webhook: {}", &body[..body.len().min(500)]))?;

    let resp_status = parsed.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if resp_status != "ok" {
        bail!("webhook drive retornou status={}: {}", resp_status, body);
    }

    let url = parsed
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let download_url = parsed
        .get("download_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if url.is_empty() {
        bail!("webhook drive não retornou URL: {}", body);
    }

    crate::log::emit(format!("[drive] Upload OK: {url}"));
    Ok(DriveUploadResult {
        url,
        id,
        download_url,
    })
}

/// Comprime um arquivo único em um ZIP em memória.
fn zip_file(path: &Path) -> Result<Vec<u8>> {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("nome de arquivo inválido"))?;

    let file_data = std::fs::read(path)
        .with_context(|| format!("falha ao ler {}", path.display()))?;

    let mut buf = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(filename, options)
            .context("falha ao iniciar entrada no zip")?;
        zip.write_all(&file_data).context("falha ao escrever no zip")?;
        zip.finish().context("falha ao finalizar zip")?;
    }
    Ok(buf)
}
