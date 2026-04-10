//! Login e gerenciamento de sessão no Peach.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

const BASE: &str = "https://latam.peachvideo.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:149.0) Gecko/20100101 Firefox/149.0";

/// Cliente HTTP com cookie store, persistindo a sessão entre requests.
pub struct PeachClient {
    pub(crate) http: Client,
}

#[derive(Debug, Deserialize)]
pub struct SessionInfo {
    pub iniciada: u32,
    #[serde(default)]
    pub nombre_usuario_activo: String,
    #[serde(default)]
    pub id_email: String,
    #[serde(default)]
    pub id_empresa: String,
    #[serde(default)]
    pub empresa_nombre: String,
    #[serde(default)]
    pub privilegios: HashMap<String, i32>,
    #[serde(default)]
    pub extension_permitida: Vec<String>,
}

impl PeachClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .gzip(true)
            .brotli(true)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("falha ao criar cliente HTTP")?;
        Ok(Self { http })
    }

    /// Faz login + redirect 307 para `secure_login.php` + valida sessão.
    pub async fn login(&self, email: &str, password: &str) -> Result<SessionInfo> {
        let url = format!("{BASE}/login/login/login");
        let res = self
            .http
            .post(&url)
            .form(&[
                ("user_email", email),
                ("user_password", password),
                ("country", "BR"),
                ("lang", "pt_BR"),
            ])
            .header("Origin", BASE)
            .header(
                "Referer",
                format!("{BASE}/login/login/index?pais=BR"),
            )
            .send()
            .await
            .context("falha no POST de login")?;

        if !res.status().is_success() {
            bail!("login retornou status {}", res.status());
        }

        // O reqwest seguiu o redirect 307 e mantém os cookies.
        // Validamos chamando session_data.
        self.session_data().await
    }

    /// Valida que a sessão atual está ativa, retornando dados do usuário.
    pub async fn session_data(&self) -> Result<SessionInfo> {
        let res = self
            .http
            .post(format!("{BASE}/app/comun/session_data.php"))
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Origin", BASE)
            .header(
                "Referer",
                format!("{BASE}/amasv/app/index_general.php"),
            )
            .header("Content-Length", "0")
            .send()
            .await
            .context("falha no POST session_data")?;

        if !res.status().is_success() {
            bail!("session_data retornou status {}", res.status());
        }

        let body = res.text().await.context("falha ao ler corpo session_data")?;
        let info: SessionInfo = serde_json::from_str(&body)
            .with_context(|| format!("falha ao parsear JSON session_data: {}", body))?;

        if info.iniciada != 1 {
            bail!("sessão não está ativa (iniciada={})", info.iniciada);
        }
        Ok(info)
    }

    /// URL base do Peach (para módulos auxiliares).
    pub fn base(&self) -> &'static str {
        BASE
    }
}
