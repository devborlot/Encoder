//! Init upload (obtenção das credenciais STS) e upload S3 multipart.

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use std::path::Path;

use super::auth::PeachClient;
use super::config::PeachConfig;

/// Credenciais AWS STS temporárias retornadas pelo Peach + metadados do envio.
#[derive(Debug, Clone)]
pub struct StsCredentials {
    pub id_envio: String,
    pub destination: String,
    pub region: String,
    pub bucket: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
}

impl StsCredentials {
    /// Extrai o `spot_id` do filename de destino retornado pelo Peach.
    /// Padrão: `<YYYYMMDD>_<spot_id>.<ext>` (ex: "20260407_2237176.mxf").
    /// Esse spot_id é o que precisa ser usado no fluxo de distribuição (`peach::send`).
    pub fn spot_id(&self) -> Option<u64> {
        let stem = std::path::Path::new(&self.destination)
            .file_stem()
            .and_then(|s| s.to_str())?;
        // Pega o segundo segmento separado por '_'
        let after_underscore = stem.split('_').nth(1)?;
        after_underscore.parse().ok()
    }
}

/// Parâmetros que vêm do VT em si (não do tenant config).
#[derive(Debug, Clone)]
pub struct UploadParams<'a> {
    pub video_path: &'a Path,
    /// Título do VT (geralmente o stem do filename — ex: "ABR_PROMO_06")
    pub pieza: &'a str,
    /// Código ANCINE sem traço (ex: "20240174220065")
    pub codigo: &'a str,
    /// Framerate como string (ex: "29.97")
    pub framerate: &'a str,
    /// Duração em segundos
    pub duration_secs: u64,
}

impl PeachClient {
    /// Chama `add_spot_upload_action.php` com os metadados e parseia
    /// a resposta HTML/JS pra extrair as credenciais STS.
    pub async fn init_upload(
        &self,
        params: &UploadParams<'_>,
        cfg: &PeachConfig,
        productora_id: &str,
    ) -> Result<StsCredentials> {
        let filename = params
            .video_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("nome de arquivo inválido"))?;

        let total_secs = params.duration_secs;
        let horas = format!("{:02}", total_secs / 3600);
        let minutos = format!("{:02}", (total_secs % 3600) / 60);
        let segundos = format!("{:02}", total_secs % 60);

        let id_marca_str = cfg.id_marca.to_string();
        let id_producto_str = cfg.id_producto.to_string();

        let url = format!(
            "{}/amasv/app/modulos/subir/add_spot_upload_action.php",
            self.base()
        );
        let query: Vec<(&str, &str)> = vec![
            ("v", "1"),
            ("pieza", params.pieza),
            ("AvisadorExtranjero", cfg.avisador_extranjero_or_zero()),
            ("avisador", cfg.avisador_id.as_str()),
            ("CNPJ_Avisador", cfg.cnpj_avisador.as_str()),
            ("id_marca", id_marca_str.as_str()),
            ("id_producto", id_producto_str.as_str()),
            ("campana", cfg.campana.as_str()),
            ("codigo", params.codigo),
            ("tipoCRT", cfg.tipo_crt.as_str()),
            ("AgenciaExtranjero", cfg.agencia_extranjero_or_zero()),
            ("AgenciaCreativa", cfg.agencia_id.as_str()),
            ("CNPJ_Creativa", cfg.cnpj_agencia.as_str()),
            ("productora", productora_id),
            ("archivo", filename),
            ("formato", cfg.formato.as_str()),
            ("aspecto", cfg.aspecto.as_str()),
            ("framerate", params.framerate),
            ("horas", horas.as_str()),
            ("minutos", minutos.as_str()),
            ("segundos", segundos.as_str()),
            ("frame", "00"),
            ("PosInicio", cfg.pos_inicio.as_str()),
            ("Vineta", cfg.vineta.as_str()),
            ("ClosedCaption", cfg.closed_caption.as_str()),
            ("TeclaSap", cfg.tecla_sap.as_str()),
            ("LenguajeSenas", cfg.lenguaje_senas.as_str()),
            ("AD", cfg.ad.as_str()),
            ("surround", cfg.surround.as_str()),
            ("audio", cfg.audio.as_str()),
            (
                "envio_exhibidor_bloqueado",
                cfg.envio_exhibidor_bloqueado.as_str(),
            ),
            ("elecciones", cfg.elecciones.as_str()),
            ("NotificarEmails", cfg.notificar_emails.as_str()),
        ];

        let res = self
            .http
            .get(&url)
            .query(&query)
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no GET add_spot_upload_action")?;

        if !res.status().is_success() {
            bail!("add_spot_upload_action retornou status {}", res.status());
        }

        let html = res
            .text()
            .await
            .context("falha ao ler resposta de add_spot_upload_action")?;

        parse_sts(&html).with_context(|| {
            format!(
                "falha ao extrair credenciais STS. Resposta (primeiros 500 chars):\n{}",
                &html.chars().take(500).collect::<String>()
            )
        })
    }
}

/// Extrai os campos do bloco JS `Filetransfer.upload({...})` retornado pelo Peach.
fn parse_sts(html: &str) -> Result<StsCredentials> {
    fn extract(html: &str, key: &str) -> Result<String> {
        let pat = format!(r#"{}\s*:\s*['"]([^'"]+)['"]"#, regex::escape(key));
        let re = Regex::new(&pat).unwrap();
        re.captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| anyhow!("campo '{}' não encontrado na resposta", key))
    }

    Ok(StsCredentials {
        id_envio: extract(html, "id_envio")?,
        destination: extract(html, "destination")?,
        region: extract(html, "region")?,
        bucket: extract(html, "Bucket")?,
        access_key_id: extract(html, "AccessKeyId")?,
        secret_access_key: extract(html, "SecretAccessKey")?,
        session_token: extract(html, "SessionToken")?,
    })
}

/// Upload S3 multipart usando as credenciais STS retornadas pelo Peach.
///
/// `on_progress(bytes_enviados, total)` é chamado a cada parte concluída.
pub async fn s3_multipart_upload<F>(
    file_path: &Path,
    sts: &StsCredentials,
    on_progress: F,
) -> Result<()>
where
    F: Fn(u64, u64),
{
    use aws_credential_types::Credentials;
    use aws_sdk_s3::config::{BehaviorVersion, Region};
    use aws_sdk_s3::primitives::ByteStream;
    use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
    use aws_sdk_s3::{Client as S3Client, Config as S3Config};

    let creds = Credentials::new(
        &sts.access_key_id,
        &sts.secret_access_key,
        Some(sts.session_token.clone()),
        None,
        "peach-sts",
    );

    let s3_config = S3Config::builder()
        .region(Region::new(sts.region.clone()))
        .credentials_provider(creds)
        .behavior_version(BehaviorVersion::latest())
        .build();

    let s3 = S3Client::from_conf(s3_config);

    let file_size = std::fs::metadata(file_path)
        .with_context(|| format!("falha ao obter metadata de {}", file_path.display()))?
        .len();

    if file_size == 0 {
        bail!("arquivo vazio: {}", file_path.display());
    }

    let chunk_size: u64 = 5 * 1024 * 1024; // 5MB

    // 1. Iniciar multipart upload
    let create = s3
        .create_multipart_upload()
        .bucket(&sts.bucket)
        .key(&sts.destination)
        .send()
        .await
        .context("falha ao iniciar multipart upload")?;

    let upload_id = create
        .upload_id()
        .ok_or_else(|| anyhow!("upload_id ausente na resposta de create_multipart_upload"))?
        .to_string();

    // 2. Upload das partes
    let mut completed_parts: Vec<CompletedPart> = Vec::new();
    let mut part_number: i32 = 1;
    let mut offset: u64 = 0;

    while offset < file_size {
        let size = std::cmp::min(chunk_size, file_size - offset);

        // Lê a parte do arquivo
        let buf = read_part(file_path, offset, size as usize)
            .with_context(|| format!("falha ao ler parte {part_number}"))?;

        let resp = s3
            .upload_part()
            .bucket(&sts.bucket)
            .key(&sts.destination)
            .upload_id(&upload_id)
            .part_number(part_number)
            .body(ByteStream::from(buf))
            .send()
            .await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                // Tenta abortar o multipart pra não deixar lixo no S3
                let _ = s3
                    .abort_multipart_upload()
                    .bucket(&sts.bucket)
                    .key(&sts.destination)
                    .upload_id(&upload_id)
                    .send()
                    .await;
                return Err(anyhow!(e)).with_context(|| format!("falha no upload da parte {part_number}"));
            }
        };

        completed_parts.push(
            CompletedPart::builder()
                .part_number(part_number)
                .e_tag(resp.e_tag().unwrap_or_default())
                .build(),
        );

        offset += size;
        on_progress(offset, file_size);
        part_number += 1;
    }

    // 3. Completar multipart upload
    let completed_upload = CompletedMultipartUpload::builder()
        .set_parts(Some(completed_parts))
        .build();

    s3.complete_multipart_upload()
        .bucket(&sts.bucket)
        .key(&sts.destination)
        .upload_id(&upload_id)
        .multipart_upload(completed_upload)
        .send()
        .await
        .context("falha ao completar multipart upload")?;

    Ok(())
}

fn read_part(file_path: &Path, offset: u64, size: usize) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(file_path)
        .with_context(|| format!("falha ao abrir {}", file_path.display()))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; size];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sts(destination: &str) -> StsCredentials {
        StsCredentials {
            id_envio: "x".into(),
            destination: destination.into(),
            region: "x".into(),
            bucket: "x".into(),
            access_key_id: "x".into(),
            secret_access_key: "x".into(),
            session_token: "x".into(),
        }
    }

    #[test]
    fn test_spot_id_extraction() {
        assert_eq!(make_sts("20260407_2237176.mxf").spot_id(), Some(2237176));
        assert_eq!(make_sts("20260408_2237867.mxf").spot_id(), Some(2237867));
        assert_eq!(make_sts("20260407_2236780.mov").spot_id(), Some(2236780));
        // Casos inválidos
        assert_eq!(make_sts("invalid.mxf").spot_id(), None);
        assert_eq!(make_sts("20260407_abc.mxf").spot_id(), None);
    }

    #[test]
    fn test_parse_sts() {
        let html = r#"
            Filetransfer.upload({
                upload_type     : 'http',
                source          : file[0],
                id_envio        : "4248547",
                destination     : "20260407_2236780.mxf",
                region          : "sa-east-1",
                Bucket          : "pro.amasv.tmp.br",
                AccessKeyId     : "ASIA53FAA6CYINRICCPC",
                SecretAccessKey : "zVx2lbGqJ8JcL+VPvAjjPAlLlC2pT30OpNc60l9H",
                SessionToken    : "FwoGZXIvYXdzEDEa..."
            });
        "#;
        let sts = parse_sts(html).unwrap();
        assert_eq!(sts.id_envio, "4248547");
        assert_eq!(sts.destination, "20260407_2236780.mxf");
        assert_eq!(sts.region, "sa-east-1");
        assert_eq!(sts.bucket, "pro.amasv.tmp.br");
        assert_eq!(sts.access_key_id, "ASIA53FAA6CYINRICCPC");
    }
}
