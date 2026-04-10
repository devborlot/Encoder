//! Consulta de status de spot no Peach.
//!
//! Após o upload S3, o Peach roda QC (Quality Control) automático no arquivo.
//! Esse processo leva alguns segundos a alguns minutos. Antes de chamar
//! `send_spots`, precisamos esperar o spot ficar pronto.
//!
//! O endpoint `inc.reel.vista_img.php` retorna a listagem de spots como HTML/JS,
//! com cada spot em uma atribuição JS no formato:
//!
//! ```js
//! Spot["2238303"] = {"SPOT_ID_PAIS":"BR", ..., "spot_se_puede_enviar":true, ...};
//! ```
//!
//! O campo `spot_se_puede_enviar` é o booleano definitivo de prontidão.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::auth::PeachClient;

/// Status simplificado de um spot no Peach.
#[derive(Debug, Clone, Deserialize)]
pub struct SpotStatus {
    #[serde(rename = "ID_SPOT")]
    pub id_spot: u64,
    #[serde(rename = "PIEZA", default)]
    pub pieza: String,
    /// QC automático: "valido" / "rechazado" / "por_verificar" / "verificando"
    #[serde(rename = "VERIFICACION", default)]
    pub verificacion: String,
    /// QC manual: "verificado" / "por_verificar" / etc.
    #[serde(rename = "VERIFICACION_MANUAL", default)]
    pub verificacion_manual: String,
    /// Flag definitivo: o Peach já liberou o spot para envio?
    #[serde(rename = "spot_se_puede_enviar", default)]
    pub spot_se_puede_enviar: bool,
}

impl PeachClient {
    /// Busca o status de um spot pela listagem `inc.reel.vista_img.php`.
    /// Retorna `None` se o spot não estiver na primeira página da listagem.
    pub async fn get_spot_status(&self, spot_id: u64) -> Result<Option<SpotStatus>> {
        let url = format!(
            "{}/amasv/app/modulos/reel/inc.reel.vista_img.php",
            self.base()
        );
        let res = self
            .http
            .get(&url)
            .query(&[
                ("vista", "vista_img"),
                ("fec_ini", ""),
                ("fec_fin", ""),
                ("usuario", ""),
                ("id_avisador", "null"),
                ("id_marca", "null"),
                ("id_producto", "null"),
                ("texto", ""),
                ("tag", ""),
                ("tagtipo", "carpeta"),
                ("page", "1"),
            ])
            .header("X-Requested-With", "XMLHttpRequest")
            .header(
                "Referer",
                format!("{}/amasv/app/index_general.php", self.base()),
            )
            .send()
            .await
            .context("falha no GET inc.reel.vista_img.php")?;

        if !res.status().is_success() {
            bail!("get_spot_status retornou status {}", res.status());
        }

        let body = res.text().await.context("falha ao ler corpo do listing")?;
        parse_spot_from_listing(&body, spot_id)
    }

    /// Polla o status do spot até estar pronto para envio (`spot_se_puede_enviar=true`)
    /// ou até dar timeout. Retorna erro se o QC rejeitar o spot.
    pub async fn wait_spot_ready(
        &self,
        spot_id: u64,
        max_attempts: usize,
        delay_secs: u64,
    ) -> Result<SpotStatus> {
        for attempt in 0..max_attempts {
            let status = self.get_spot_status(spot_id).await?;

            match status {
                None => {
                    crate::log::emit(format!(
                        "[peach] Spot {spot_id} ainda não visível na listagem (tentativa {}/{})",
                        attempt + 1,
                        max_attempts
                    ));
                }
                Some(s) => {
                    if s.verificacion == "rechazado" {
                        bail!(
                            "Spot {} ({}) foi REJEITADO pelo QC do Peach (VERIFICACION={}, MANUAL={})",
                            s.id_spot,
                            s.pieza,
                            s.verificacion,
                            s.verificacion_manual
                        );
                    }

                    if s.spot_se_puede_enviar {
                        crate::log::emit(format!(
                            "[peach] Spot {} ({}) PRONTO para envio. VERIFICACION={}, MANUAL={}",
                            s.id_spot, s.pieza, s.verificacion, s.verificacion_manual
                        ));
                        return Ok(s);
                    }

                    crate::log::emit(format!(
                        "[peach] Spot {} ({}) aguardando QC. VERIFICACION={}, MANUAL={} (tentativa {}/{})",
                        s.id_spot,
                        s.pieza,
                        s.verificacion,
                        s.verificacion_manual,
                        attempt + 1,
                        max_attempts
                    ));
                }
            }

            if attempt + 1 < max_attempts {
                tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            }
        }
        bail!(
            "Spot {} não ficou pronto após {} tentativas (~{} min). Tente enviar manualmente mais tarde.",
            spot_id,
            max_attempts,
            (max_attempts as u64 * delay_secs) / 60
        )
    }
}

/// Procura o objeto JS `Spot["<id>"] = {...};` no body e parseia o JSON.
fn parse_spot_from_listing(body: &str, spot_id: u64) -> Result<Option<SpotStatus>> {
    let needle = format!(r#"Spot["{spot_id}"] = "#);
    let Some(start) = body.find(&needle) else {
        return Ok(None);
    };
    let json_start = start + needle.len();
    let tail = &body[json_start..];

    // Usa o Deserializer pra detectar o fim do objeto JSON dentro do blob
    let mut de = serde_json::Deserializer::from_str(tail);
    let value = serde_json::Value::deserialize(&mut de)
        .with_context(|| format!("falha ao parsear JSON do spot {spot_id}"))?;

    let status: SpotStatus =
        serde_json::from_value(value).context("falha ao desserializar SpotStatus")?;
    Ok(Some(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_spot_from_listing() {
        let body = r#"
            <script>
            Spot["2238303"] = {"SPOT_ID_PAIS":"BR","ID_SPOT":2238303,"PIEZA":"ABR_PROMO_10","VERIFICACION":"valido","VERIFICACION_MANUAL":"por_verificar","spot_se_puede_enviar":true,"foo":{"nested":"yes"}};
            Spot["2238310"] = {"ID_SPOT":2238310,"PIEZA":"ABR_PROMO_11","VERIFICACION":"por_verificar","VERIFICACION_MANUAL":"por_verificar","spot_se_puede_enviar":false};
            </script>
        "#;
        let s = parse_spot_from_listing(body, 2238303).unwrap().unwrap();
        assert_eq!(s.id_spot, 2238303);
        assert_eq!(s.pieza, "ABR_PROMO_10");
        assert_eq!(s.verificacion, "valido");
        assert!(s.spot_se_puede_enviar);

        let s = parse_spot_from_listing(body, 2238310).unwrap().unwrap();
        assert_eq!(s.id_spot, 2238310);
        assert_eq!(s.verificacion, "por_verificar");
        assert!(!s.spot_se_puede_enviar);

        // Spot inexistente
        assert!(parse_spot_from_listing(body, 999).unwrap().is_none());
    }
}
