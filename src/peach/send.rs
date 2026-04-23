//! Distribuição (envio) de spots já uploadados para destinos TV.
//!
//! Fluxo (engenharia reversa do portal latam.peachvideo.com — ver `peach-rest-flow.md`):
//!
//! 1. **Validar** combinações spot×destino (`/amasv/public/delivery/validate`)
//! 2. **Confirmar** envio (gera HTML, valida status — `enviar_confirmar.php`)
//! 3. **Executar** envio (`enviar_confirma_accion.php`)

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

use super::auth::PeachClient;
use super::config::PeachConfig;

/// Resposta do endpoint `/amasv/public/delivery/validate`.
#[derive(Debug, Deserialize)]
pub struct ValidateResponse {
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "Envios", default)]
    pub envios: serde_json::Value,
    /// Body bruto pra debug (não vem da API, é preenchido pelo parser).
    #[serde(skip)]
    pub raw_body: String,
}

/// Pedido de envio: lista de spots × lista de destinos por codec.
#[derive(Debug, Clone)]
pub struct SendRequest<'a> {
    pub spot_ids: &'a [u64],
    pub destinos_hd: &'a [String],
    pub destinos_sd: &'a [String],
}

impl<'a> SendRequest<'a> {
    /// Validação básica antes de chamar a API.
    pub fn check(&self) -> Result<()> {
        if self.spot_ids.is_empty() {
            bail!("nenhum spot informado para envio");
        }
        if self.destinos_hd.is_empty() && self.destinos_sd.is_empty() {
            bail!("nenhum destino informado");
        }
        Ok(())
    }
}

impl PeachClient {
    /// Pré-etapa: carrega uma lista de destinos salva no portal.
    /// Isso seta contexto de sessão no servidor que torna o validate mais permissivo
    /// (resolve o erro "Exhibidoras obrigatórios" quando há sub-emisoras).
    pub async fn load_destinos_list(&self, id_lista: u32) -> Result<()> {
        let url = format!(
            "{}/amasv/app/modulos/enviar/lista_destinos.php",
            self.base()
        );
        let res = self
            .http
            .post(&url)
            .form(&[("accion", "buscar"), ("id_lista", &id_lista.to_string())])
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no POST lista_destinos")?;

        if !res.status().is_success() {
            bail!("lista_destinos retornou status {}", res.status());
        }
        crate::log::emit(format!(
            "[peach] lista_destinos(id_lista={id_lista}) carregada"
        ));
        Ok(())
    }

    /// Pré-etapa: verifica quantidade de destinos com sub-emisoras obrigatórias.
    /// Chama `exhibidor_emisoras.php` conforme o fluxo do portal.
    pub async fn check_exhibidor_emisoras(&self, has_record: bool, has_redetv: bool) -> Result<u32> {
        let url = format!(
            "{}/amasv/app/modulos/enviar/exhibidor_emisoras.php",
            self.base()
        );
        let res = self
            .http
            .post(&url)
            .form(&[
                ("accion", "getCantidadDestinosEmisoras"),
                (
                    "destino_record",
                    if has_record { "true" } else { "false" },
                ),
                (
                    "destino_redetv",
                    if has_redetv { "true" } else { "false" },
                ),
            ])
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no POST exhibidor_emisoras")?;

        let body = res.text().await.unwrap_or_default();
        let count: u32 = body.trim().parse().unwrap_or(0);
        crate::log::emit(format!(
            "[peach] exhibidor_emisoras: cantidad={count}"
        ));
        Ok(count)
    }

    /// Etapa 1: valida combinações spot×destino.
    /// Retorna `Ok(ValidateResponse)` com `Status:"Success"` se tudo ok.
    pub async fn validate_delivery(&self, req: &SendRequest<'_>) -> Result<ValidateResponse> {
        req.check()?;

        // Monta query string manualmente porque reqwest não serializa bem arrays repetidos
        let mut url = format!("{}/amasv/public/delivery/validate?", self.base());
        for sid in req.spot_ids {
            url.push_str(&format!("spots%5B%5D={sid}&"));
        }
        for d in req.destinos_hd.iter().chain(req.destinos_sd.iter()) {
            url.push_str(&format!(
                "destinos%5B%5D={}&",
                urlencoding_minimal(d)
            ));
        }
        url.push_str("tipoExhibidor=");

        let res = self
            .http
            .get(&url)
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no GET /delivery/validate")?;

        if !res.status().is_success() {
            bail!("validate retornou status {}", res.status());
        }

        let body = res
            .text()
            .await
            .context("falha ao ler corpo de /delivery/validate")?;

        let mut parsed: ValidateResponse = serde_json::from_str(&body).with_context(|| {
            format!("falha ao parsear validate JSON: {}", &body[..body.len().min(500)])
        })?;
        parsed.raw_body = body;
        Ok(parsed)
    }

    /// Etapa 2: confirma o envio (server gera HTML do diálogo de confirmação).
    /// Validação: status 200. Não usamos o HTML retornado.
    pub async fn confirm_send(&self, req: &SendRequest<'_>) -> Result<()> {
        req.check()?;

        let select_spot = format_spots(req.spot_ids);
        let destinos_hd = format_destinos_confirm(req.destinos_hd, "HD");
        let destinos_sd = format_destinos_confirm(req.destinos_sd, "SD");

        let body = vec![
            ("v", "1"),
            ("email_confirmacion_envio", ""),
            ("caso_downconvert", "false"),
            ("selPais", "BR"),
            ("tipoEmp", "1"),
            ("select_spot", &select_spot),
            ("destinos_agencias", ""),
            ("destinos_SD", &destinos_sd),
            ("destinos_HD", &destinos_hd),
            ("cantidad_emisoras_globo", "0"),
            ("comercializadoras", ""),
            ("tipoExhibidor", ""),
        ];

        let url = format!(
            "{}/amasv/app/modulos/enviar/enviar_confirmar.php",
            self.base()
        );
        let res = self
            .http
            .post(&url)
            .form(&body)
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no POST enviar_confirmar")?;

        if !res.status().is_success() {
            bail!("enviar_confirmar retornou status {}", res.status());
        }
        Ok(())
    }

    /// Etapa 3: executa o envio. Retorna sumário textual.
    pub async fn execute_send(&self, req: &SendRequest<'_>) -> Result<String> {
        req.check()?;

        let s_param = format_spots(req.spot_ids);
        // Formato action: <empresa>||<HD/SD>; (sub-emisora vazio, fluxo simples)
        let mut e_param = String::new();
        for d in req.destinos_hd {
            e_param.push_str(&format!("{d}||HD;"));
        }
        for d in req.destinos_sd {
            e_param.push_str(&format!("{d}||SD;"));
        }

        let body = vec![
            ("s", s_param.as_str()),
            ("e", e_param.as_str()),
            ("comercializadoras", ""),
        ];

        let url = format!(
            "{}/amasv/app/modulos/enviar/enviar_confirma_accion.php?email_aviso_envio=&DC=false&aux_envio=1&id_req=&id_material=&selPais=BR",
            self.base()
        );
        let res = self
            .http
            .post(&url)
            .form(&body)
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no POST enviar_confirma_accion")?;

        if !res.status().is_success() {
            bail!("enviar_confirma_accion retornou status {}", res.status());
        }

        let body = res.text().await.unwrap_or_default();
        // A resposta tipicamente é um script JS com goReportes(...). Sucesso = 200.
        let summary = format!(
            "Distribuído: {} spot(s) → {} destino(s)",
            req.spot_ids.len(),
            req.destinos_hd.len() + req.destinos_sd.len(),
        );
        // Se body trouxer info de erro óbvia, anexa
        if body.to_lowercase().contains("error") || body.to_lowercase().contains("erro") {
            Ok(format!(
                "{summary}\n[server response]: {}",
                &body[..body.len().min(500)]
            ))
        } else {
            Ok(summary)
        }
    }

    /// Helper high-level: aguarda QC + valida + confirma + executa.
    ///
    /// Fluxo:
    /// 1. Para cada spot, polla `wait_spot_ready` até `spot_se_puede_enviar=true`
    ///    (ou até timeout / rejeição pelo QC).
    /// 2. Chama `validate_delivery`. Se falhar só com erros de QC residuais,
    ///    retenta com backoff. Se falhar com erro estrutural (Exhibidoras), aborta.
    /// 3. Confirma e executa o envio.
    pub async fn send_spots(&self, req: &SendRequest<'_>, cfg: &PeachConfig) -> Result<String> {
        // Configuração de polling de status do spot
        const SPOT_READY_MAX_ATTEMPTS: usize = 40; // 40 * 15s = 10 min
        const SPOT_READY_DELAY_SECS: u64 = 15;
        // Configuração de retry de validate (pra QC residual)
        const MAX_QC_RETRIES: usize = 10;
        const QC_RETRY_DELAY_SECS: u64 = 30;

        // Etapa 0: carrega listas de destinos (seta contexto de sessão no servidor)
        for &id_lista in &cfg.destinos.id_listas {
            self.load_destinos_list(id_lista).await?;
        }

        // Etapa 0b: check exhibidor_emisoras (necessário pro fluxo do portal)
        self.check_exhibidor_emisoras(false, false).await?;

        // Etapa 1: aguarda cada spot ficar pronto pra envio
        for &spot_id in req.spot_ids {
            crate::log::emit(format!("[peach] Aguardando spot {spot_id} ficar pronto..."));
            self.wait_spot_ready(spot_id, SPOT_READY_MAX_ATTEMPTS, SPOT_READY_DELAY_SECS)
                .await?;
        }

        // Etapa 2-3: validate (com retry pra QC residual) + confirm + execute
        for attempt in 0..=MAX_QC_RETRIES {
            let val = self.validate_delivery(req).await?;

            if val.status == "Success" {
                crate::log::emit("[peach] validate OK");
                self.confirm_send(req).await?;
                crate::log::emit("[peach] confirm_send OK");
                let summary = self.execute_send(req).await?;
                crate::log::emit(format!("[peach] {summary}"));
                return Ok(summary);
            }

            // Warning: o Peach tem observações (QC, etc.) mas permite envio.
            // Log dos warnings e prossegue como Success.
            if val.status == "Warning" {
                let warnings = extract_warnings_summary(&val.envios);
                crate::log::emit(format!(
                    "[peach] validate OK (com warnings): {warnings}"
                ));
                self.confirm_send(req).await?;
                crate::log::emit("[peach] confirm_send OK");
                let summary = self.execute_send(req).await?;
                crate::log::emit(format!("[peach] {summary}"));
                return Ok(summary);
            }

            // Analisa o tipo de erro
            let analysis = analyze_validate_errors(&val.envios);

            crate::log::emit(format!(
                "[peach] validate FALHOU (tentativa {}/{}). QC errors: {}, outros: {}",
                attempt + 1,
                MAX_QC_RETRIES + 1,
                analysis.qc_errors,
                analysis.non_qc_errors
            ));

            if analysis.non_qc_errors > 0 {
                // Erros não-QC (ex: "Exhibidoras obrigatórias"). Validate é mais
                // rigoroso que o action — o servidor aceita o envio mesmo sem
                // sub-emisoras especificadas no body (preenche com defaults).
                // Loga warning e PROSSEGUE pro confirm/execute.
                crate::log::emit(format!(
                    "[peach] validate retornou Status=Error com {} aviso(s) não-QC. Prosseguindo mesmo assim (action é tolerante).\nDetalhes:\n{}",
                    analysis.non_qc_errors,
                    val.raw_body
                ));
                // Pula o retry de QC, vai direto pro confirm/execute
                self.confirm_send(req).await?;
                crate::log::emit("[peach] confirm_send OK");
                let summary = self.execute_send(req).await?;
                crate::log::emit(format!("[peach] {summary}"));
                return Ok(summary);
            }

            if analysis.qc_errors == 0 {
                // Status=Error mas sem erros listados? Estranho. Aborta.
                bail!(
                    "validate retornou status={} sem detalhes:\n{}",
                    val.status,
                    val.raw_body
                );
            }

            if attempt == MAX_QC_RETRIES {
                bail!(
                    "QC do Peach ainda não passou após {} tentativas (~{} min). Tente enviar manualmente mais tarde.\n{}",
                    MAX_QC_RETRIES + 1,
                    (MAX_QC_RETRIES as u64 * QC_RETRY_DELAY_SECS) / 60,
                    val.raw_body
                );
            }

            crate::log::emit(format!(
                "[peach] QC ainda não pronto. Aguardando {}s antes da próxima tentativa ({}/{})...",
                QC_RETRY_DELAY_SECS,
                attempt + 2,
                MAX_QC_RETRIES + 1
            ));
            tokio::time::sleep(tokio::time::Duration::from_secs(QC_RETRY_DELAY_SECS)).await;
        }
        bail!("loop de retry esgotado")
    }
}

/// Resultado da análise de erros do validate.
struct ValidateErrorAnalysis {
    qc_errors: usize,
    non_qc_errors: usize,
}

/// Extrai um sumário dos warnings (ex: "QC: Spot contém observações QC; ...")
fn extract_warnings_summary(envios: &serde_json::Value) -> String {
    let Some(obj) = envios.as_object() else {
        return "(sem detalhes)".to_string();
    };
    let mut msgs = Vec::new();
    for (dest_key, envio) in obj {
        let Some(warnings) = envio.get("Warnings").and_then(|w| w.as_array()) else {
            continue;
        };
        for w in warnings {
            let campo = w.get("Campo").and_then(|c| c.as_str()).unwrap_or("");
            let msg = w.get("Mensaje").and_then(|m| m.as_str()).unwrap_or("");
            msgs.push(format!("{dest_key} [{campo}]: {msg}"));
        }
    }
    if msgs.is_empty() {
        "(sem warnings detalhados)".to_string()
    } else {
        msgs.join(" | ")
    }
}

/// Conta erros por tipo. QC errors são recuperáveis (timing), outros não.
fn analyze_validate_errors(envios: &serde_json::Value) -> ValidateErrorAnalysis {
    let mut qc = 0;
    let mut non_qc = 0;
    let Some(obj) = envios.as_object() else {
        return ValidateErrorAnalysis {
            qc_errors: 0,
            non_qc_errors: 0,
        };
    };
    for (_, envio) in obj {
        let Some(errores) = envio.get("Errores").and_then(|e| e.as_array()) else {
            continue;
        };
        for err in errores {
            let campo = err.get("Campo").and_then(|c| c.as_str()).unwrap_or("");
            if campo == "QC" {
                qc += 1;
            } else {
                non_qc += 1;
            }
        }
    }
    ValidateErrorAnalysis {
        qc_errors: qc,
        non_qc_errors: non_qc,
    }
}

// ----------------- Helpers de formato -----------------

fn format_spots(ids: &[u64]) -> String {
    // "id1;id2;id3;"
    ids.iter().map(|id| format!("{id};")).collect()
}

fn format_destinos_confirm(empresas: &[String], codec: &str) -> String {
    // "BR_GLOBO_112|HD;BR_GLOBO_79|HD;"
    empresas.iter().map(|e| format!("{e}|{codec};")).collect()
}

/// URL encode mínimo para os valores de destinos[] (apenas chars problemáticos).
// ----------------- Log CSV -----------------

/// Registro de um envio bem-sucedido.
#[derive(Debug, Clone, Default)]
pub struct SendLogEntry {
    pub timestamp: String,
    pub pieza: String,
    pub codigo: String,
    pub spot_id: u64,
    pub destinos: String,
    pub id_envio: String,
    /// URL do MP4 agência no Google Drive (se compartilhado).
    pub agencia_url: String,
}

/// Grava uma linha no CSV de log de envios.
/// Cria o arquivo com cabeçalho se não existir, ou appenda se já existir.
pub fn append_send_log(output_dir: &Path, entry: &SendLogEntry) -> Result<()> {
    use std::io::Write;
    let csv_path = output_dir.join("envios_log.csv");
    let file_exists = csv_path.exists();

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&csv_path)
        .with_context(|| format!("falha ao abrir {}", csv_path.display()))?;

    if !file_exists {
        writeln!(
            file,
            "data_hora,titulo,codigo_crt,spot_id,destinos,id_envio,agencia_url"
        )?;
    }

    // Escapa campos com vírgula
    let destinos_escaped = if entry.destinos.contains(',') {
        format!("\"{}\"", entry.destinos)
    } else {
        entry.destinos.clone()
    };

    writeln!(
        file,
        "{},{},{},{},{},{},{}",
        entry.timestamp,
        entry.pieza,
        entry.codigo,
        entry.spot_id,
        destinos_escaped,
        entry.id_envio,
        entry.agencia_url
    )?;

    crate::log::emit(format!(
        "[peach] Log salvo em {}",
        csv_path.display()
    ));
    Ok(())
}

/// Envia o registro de envio para um webhook (Google Apps Script → Google Sheets).
/// Best-effort: não falha se o webhook não responder.
pub async fn post_webhook(webhook_url: &str, entry: &SendLogEntry, cliente: &str) {
    if webhook_url.is_empty() {
        return;
    }
    let payload = serde_json::json!({
        "timestamp": entry.timestamp,
        "pieza": entry.pieza,
        "codigo": entry.codigo,
        "spot_id": entry.spot_id,
        "destinos": entry.destinos,
        "id_envio": entry.id_envio,
        "cliente": cliente,
        "agencia_url": entry.agencia_url,
    });

    crate::log::emit(format!("[peach] Enviando registro pro webhook..."));

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            crate::log::emit(format!("[peach] Aviso: falha ao criar HTTP client pro webhook: {e}"));
            return;
        }
    };

    match client
        .post(webhook_url)
        .json(&payload)
        .send()
        .await
    {
        Ok(res) => {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            if status.is_success() || status.is_redirection() {
                crate::log::emit(format!("[peach] Webhook OK ({})", status));
            } else {
                crate::log::emit(format!(
                    "[peach] Aviso: webhook retornou {}: {}",
                    status,
                    &body[..body.len().min(200)]
                ));
            }
        }
        Err(e) => {
            crate::log::emit(format!("[peach] Aviso: falha no webhook: {e}"));
        }
    }
}

fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => out.push_str(&format!("%{:02X}", c as u32)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_spots() {
        assert_eq!(format_spots(&[2237867, 2237865]), "2237867;2237865;");
        assert_eq!(format_spots(&[123]), "123;");
        assert_eq!(format_spots(&[]), "");
    }

    #[test]
    fn test_format_destinos_confirm() {
        let empresas = vec!["BR_GLOBO_112".to_string(), "BR_GLOBO_79".to_string()];
        assert_eq!(
            format_destinos_confirm(&empresas, "HD"),
            "BR_GLOBO_112|HD;BR_GLOBO_79|HD;"
        );
        assert_eq!(format_destinos_confirm(&[], "HD"), "");
    }

    #[test]
    fn test_urlencoding_minimal() {
        assert_eq!(urlencoding_minimal("BR_GLOBO_112"), "BR_GLOBO_112");
        assert_eq!(urlencoding_minimal("BR-1230"), "BR-1230");
        assert_eq!(urlencoding_minimal("a b"), "a%20b");
    }

    #[test]
    fn test_analyze_validate_errors() {
        // Só QC errors (recuperável)
        let envios: serde_json::Value = serde_json::from_str(
            r#"{
                "2238326@BR1230": {
                    "Errores": [{"Campo":"QC","Mensaje":"Spot com QC Error"}]
                }
            }"#,
        )
        .unwrap();
        let r = analyze_validate_errors(&envios);
        assert_eq!(r.qc_errors, 1);
        assert_eq!(r.non_qc_errors, 0);

        // QC + Exhibidoras (não recuperável)
        let envios: serde_json::Value = serde_json::from_str(
            r#"{
                "2238326@BR_GLOBO_112": {
                    "Errores": [
                        {"Campo":"QC","Mensaje":"Spot com QC Error"},
                        {"Campo":"Exhibidoras","Mensaje":"Exibidoras obrigatórios"}
                    ]
                }
            }"#,
        )
        .unwrap();
        let r = analyze_validate_errors(&envios);
        assert_eq!(r.qc_errors, 1);
        assert_eq!(r.non_qc_errors, 1);

        // Múltiplos destinos, mistos
        let envios: serde_json::Value = serde_json::from_str(
            r#"{
                "spot1@dest1": { "Errores": [{"Campo":"QC","Mensaje":"x"}] },
                "spot1@dest2": { "Errores": [{"Campo":"Exhibidoras","Mensaje":"y"}] }
            }"#,
        )
        .unwrap();
        let r = analyze_validate_errors(&envios);
        assert_eq!(r.qc_errors, 1);
        assert_eq!(r.non_qc_errors, 1);
    }

    #[test]
    fn test_send_request_check() {
        let empty: Vec<String> = vec![];
        let hd = vec!["BR_GLOBO_112".to_string()];
        // OK
        assert!(SendRequest {
            spot_ids: &[123],
            destinos_hd: &hd,
            destinos_sd: &empty,
        }
        .check()
        .is_ok());
        // Sem spots
        assert!(SendRequest {
            spot_ids: &[],
            destinos_hd: &hd,
            destinos_sd: &empty,
        }
        .check()
        .is_err());
        // Sem destinos
        assert!(SendRequest {
            spot_ids: &[123],
            destinos_hd: &empty,
            destinos_sd: &empty,
        }
        .check()
        .is_err());
    }
}
