// Em release, sem console (evita stderr noise de DLLs injetadas no processo)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Context;
use clap::Parser;
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use encoder_lib::{config, encoder, metadata, peach, slate};

const MAX_LOG_LINES: usize = 500;

// --- Persistent GUI state ---

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct GuiState {
    #[serde(default)]
    last_client: String, // "" = cliente padrão (raiz)
    #[serde(default = "default_true")]
    render_mxf: bool,
    #[serde(default = "default_true")]
    render_mp4: bool,
    #[serde(default)]
    keep_mxf_after_send: bool,
    #[serde(default = "default_true")]
    distribute_after_upload: bool,
    #[serde(default)]
    share_to_drive: bool,
    #[serde(default)]
    output_per_client: HashMap<String, String>,
    /// Última seleção de destinos por cliente (key = nome do cliente, "" = padrão).
    /// Valor = lista de IDs marcados.
    #[serde(default)]
    selected_destinos_per_client: HashMap<String, Vec<String>>,
}

fn default_true() -> bool {
    true
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            last_client: String::new(),
            render_mxf: true,
            render_mp4: true,
            keep_mxf_after_send: false,
            distribute_after_upload: true,
            share_to_drive: false,
            output_per_client: HashMap::new(),
            selected_destinos_per_client: HashMap::new(),
        }
    }
}

impl GuiState {
    fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("gui_state.toml")
    }
    fn load(config_dir: &Path) -> Self {
        let p = Self::path(config_dir);
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self, config_dir: &Path) {
        let p = Self::path(config_dir);
        if let Ok(s) = toml::to_string(self) {
            let _ = std::fs::write(&p, s);
        }
    }
}

#[derive(Parser)]
#[command(name = "encoder-gui", about = "GUI para claquete + encoding MXF XDCAM HD422")]
struct GuiArgs {
    /// Caminho do vídeo MP4 de entrada
    video: Option<PathBuf>,

    /// Perfil de cliente (subpasta em config/)
    #[arg(short = 'C', long)]
    client: Option<String>,
}

// --- Messages from background thread ---

enum EncoderMessage {
    /// Linha de log pra exibir no painel
    Log(String),
    /// Status textual (ex: "Encodando MXF...", "Enviando para Peach...")
    Status(String),
    /// Progresso do upload (sent, total, label)
    UploadProgress(u64, u64),
    /// Concluído com sucesso
    Finished(String),
    /// Erro
    Error(String),
}

// --- App State ---

struct EncoderApp {
    // Client selection
    config_dir: PathBuf,
    available_clients: Vec<String>,
    selected_client: Option<String>, // None = config raiz

    // Config
    codes: HashMap<u32, String>,
    config_error: Option<String>,
    peach_cfg: Option<peach::PeachConfig>,
    peach_creds_error: Option<String>,

    // Video
    video_path: Option<PathBuf>,
    video_meta: Option<metadata::VideoMetadata>,
    probe_error: Option<String>,

    // Slate fields (editable)
    titulo: String,
    produto: String,
    duracao: String,
    produtora: String,
    agencia: String,
    anunciante: String,
    diretor: String,
    registro: String,
    data: String,

    // Output
    output_dir: String,
    default_output: String,

    // Render options
    render_mxf: bool,
    render_mp4: bool,
    keep_mxf_after_send: bool,
    distribute_after_upload: bool,
    share_to_drive: bool,
    /// IDs dos destinos atualmente marcados (subconjunto do peach.destinos do cliente).
    selected_destinos: std::collections::HashSet<String>,

    // Persistência
    state: GuiState,

    // Registro warning
    registro_warning: Option<String>,

    // Encoding state
    encoding: bool,
    status_text: String,
    upload_progress: Option<(u64, u64)>,
    result_message: Option<(bool, String)>, // (success, message)
    rx: Option<mpsc::Receiver<EncoderMessage>>,

    // Log panel
    log_lines: VecDeque<String>,
    show_log: bool,
}

impl EncoderApp {
    fn new(initial_video: Option<PathBuf>, initial_client: Option<String>) -> Self {
        let config_dir = find_config_dir();
        let available_clients = config::list_clients(&config_dir);

        // Carrega state persistente
        let state = GuiState::load(&config_dir);

        // Cliente: arg CLI > último usado > nenhum
        let selected_client = initial_client
            .or_else(|| {
                if state.last_client.is_empty() {
                    None
                } else {
                    Some(state.last_client.clone())
                }
            })
            .filter(|c| available_clients.contains(c));

        let render_mxf = state.render_mxf;
        let render_mp4 = state.render_mp4;
        let keep_mxf_after_send = state.keep_mxf_after_send;
        let distribute_after_upload = state.distribute_after_upload;
        let share_to_drive = state.share_to_drive;

        let mut app = Self {
            config_dir: config_dir.clone(),
            available_clients,
            selected_client: selected_client.clone(),
            codes: HashMap::new(),
            config_error: None,
            peach_cfg: None,
            peach_creds_error: None,
            video_path: None,
            video_meta: None,
            probe_error: None,
            titulo: String::new(),
            produto: String::new(),
            duracao: String::new(),
            produtora: String::new(),
            agencia: String::new(),
            anunciante: String::new(),
            diretor: String::new(),
            registro: String::new(),
            data: chrono::Datelike::year(&chrono::Local::now()).to_string(),
            output_dir: String::new(),
            default_output: String::new(),
            render_mxf,
            render_mp4,
            keep_mxf_after_send,
            distribute_after_upload,
            share_to_drive,
            selected_destinos: std::collections::HashSet::new(),
            state,
            registro_warning: None,
            encoding: false,
            status_text: String::new(),
            upload_progress: None,
            result_message: None,
            rx: None,
            log_lines: VecDeque::with_capacity(MAX_LOG_LINES),
            show_log: true,
        };

        app.reload_config();

        if let Some(path) = initial_video {
            app.load_video(path);
        }

        app
    }

    /// Adiciona uma linha ao painel de log (com cap circular).
    fn push_log(&mut self, line: String) {
        if self.log_lines.len() >= MAX_LOG_LINES {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(line);
    }

    /// Persiste o state atual em disco.
    fn save_state(&mut self) {
        self.state.last_client = self.selected_client.clone().unwrap_or_default();
        self.state.render_mxf = self.render_mxf;
        self.state.render_mp4 = self.render_mp4;
        self.state.keep_mxf_after_send = self.keep_mxf_after_send;
        self.state.distribute_after_upload = self.distribute_after_upload;
        self.state.share_to_drive = self.share_to_drive;

        let key = self.selected_client.clone().unwrap_or_default();

        // Salva o output atual pro cliente atual (se diferente do default da config)
        if !self.output_dir.is_empty() && self.output_dir != self.default_output {
            self.state
                .output_per_client
                .insert(key.clone(), self.output_dir.clone());
        }

        // Salva seleção de destinos pro cliente atual
        if let Some(cfg) = &self.peach_cfg {
            let available: Vec<String> = cfg.destinos.all_ids();
            let selected: Vec<String> = available
                .into_iter()
                .filter(|id| self.selected_destinos.contains(id))
                .collect();
            self.state
                .selected_destinos_per_client
                .insert(key, selected);
        }

        self.state.save(&self.config_dir);
    }

    /// Localiza o MXF a ser enviado:
    /// 1. Se o arquivo selecionado já é .mxf → usa ele
    /// 2. Senão tenta `output_dir/<titulo>.mxf`
    fn find_mxf_for_send(&self) -> Option<PathBuf> {
        let selected = self.video_path.as_ref()?;
        let is_mxf = selected
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mxf"))
            .unwrap_or(false);
        if is_mxf {
            return Some(selected.clone());
        }
        if self.titulo.is_empty() || self.output_dir.is_empty() {
            return None;
        }
        let candidate = PathBuf::from(&self.output_dir).join(format!("{}.mxf", self.titulo));
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    }

    fn reload_config(&mut self) {
        self.config_error = None;
        self.peach_cfg = None;
        let client_ref = self.selected_client.as_deref();

        let mut defaults: Option<config::Defaults> = None;
        self.codes = HashMap::new();

        // Carrega defaults com bloco [peach] opcional
        match peach::config::DefaultsWithPeach::load(&self.config_dir, client_ref) {
            Ok(d) => {
                self.peach_cfg = d.peach.clone();
                defaults = Some(d.base);
            }
            Err(e) => self.config_error = Some(format!("Erro ao carregar defaults.toml: {e}")),
        }

        match config::load_codes_for(&self.config_dir, client_ref) {
            Ok(c) => self.codes = c,
            Err(e) => {
                let msg = format!("Erro ao carregar codes.toml: {e}");
                self.config_error = Some(match self.config_error.take() {
                    Some(prev) => format!("{prev}\n{msg}"),
                    None => msg,
                });
            }
        }

        // Verifica credenciais Peach (sem falhar se ausentes — só desabilita o botão)
        self.peach_creds_error = match peach::PeachCredentials::load(&self.config_dir) {
            Ok(_) => None,
            Err(e) => Some(format!("{e}")),
        };

        let (produto, produtora, agencia, anunciante, diretor) = match &defaults {
            Some(d) => (
                d.produto.clone(),
                d.produtora.clone(),
                d.agencia.clone(),
                d.anunciante.clone(),
                d.diretor.clone(),
            ),
            None => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ),
        };

        self.produto = produto;
        self.produtora = produtora;
        self.agencia = agencia;
        self.anunciante = anunciante;
        self.diretor = diretor;

        self.default_output = defaults
            .as_ref()
            .map(|d| d.output.clone())
            .unwrap_or_default();

        // Override do state (último output usado pra esse cliente)
        let key = self.selected_client.clone().unwrap_or_default();
        self.output_dir = self
            .state
            .output_per_client
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.default_output.clone());

        // Carrega seleção de destinos do state, ou marca todos por default
        self.selected_destinos.clear();
        if let Some(cfg) = &self.peach_cfg {
            let available_ids: Vec<String> = cfg.destinos.all_ids();
            let saved = self.state.selected_destinos_per_client.get(&key);
            for id in &available_ids {
                let should_select = match saved {
                    Some(list) => list.contains(id),
                    None => true, // sem state → marca todos por default
                };
                if should_select {
                    self.selected_destinos.insert(id.clone());
                }
            }
        }

        // Re-resolve registro if a video is loaded
        if self.video_path.is_some() {
            self.resolve_current_registro();
        }
    }

    fn resolve_current_registro(&mut self) {
        self.registro_warning = None;
        let filename = self
            .video_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if filename.is_empty() {
            return;
        }
        let code = config::extract_code_from_filename(filename);
        match code {
            Some(c) => match config::lookup_registro(c, &self.codes) {
                Some(reg) => self.registro = reg,
                None => {
                    self.registro.clear();
                    self.registro_warning = Some(format!(
                        "Código {c} não encontrado na tabela de registros"
                    ));
                }
            },
            None => {
                self.registro.clear();
                self.registro_warning = Some(
                    "Não foi possível extrair código do nome do arquivo".to_string(),
                );
            }
        }
    }

    fn select_video(&mut self) {
        let file = rfd::FileDialog::new()
            .add_filter("Vídeo", &["mp4", "mov", "avi", "mkv", "mxf"])
            .set_title("Selecionar vídeo")
            .pick_file();

        if let Some(path) = file {
            self.load_video(path);
        }
    }

    fn load_video(&mut self, path: PathBuf) {
        self.probe_error = None;
        self.result_message = None;
        self.registro_warning = None;

        // Probe metadata
        match metadata::probe(&path) {
            Ok(meta) => {
                // Auto-fill title from filename
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("video");
                let stem = Path::new(filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(filename);
                self.titulo = stem.to_string();
                self.duracao = meta.duration_display();

                // Resolve registro from codes table
                let code = config::extract_code_from_filename(filename);
                match code {
                    Some(c) => match config::lookup_registro(c, &self.codes) {
                        Some(reg) => self.registro = reg,
                        None => {
                            self.registro.clear();
                            self.registro_warning = Some(format!(
                                "Código {c} não encontrado na tabela de registros"
                            ));
                        }
                    },
                    None => {
                        self.registro.clear();
                        self.registro_warning = Some(
                            "Não foi possível extrair código do nome do arquivo".to_string(),
                        );
                    }
                }

                self.video_meta = Some(meta);
            }
            Err(e) => {
                self.probe_error = Some(format!("Erro ao ler metadados: {e}"));
                self.video_meta = None;
            }
        }

        // Output dir: NÃO sobrescreve se já tem (vem do state ou config).
        // Só preenche se estiver vazio.
        if self.output_dir.is_empty() {
            if !self.default_output.is_empty() {
                self.output_dir = self.default_output.clone();
            } else if let Some(parent) = path.parent() {
                self.output_dir = parent.display().to_string();
            }
        }

        self.video_path = Some(path);
    }

    /// Inicia encoding (e opcionalmente upload pro Peach depois).
    fn start_encoding(&mut self, ctx: &egui::Context, then_upload: bool) {
        let video_path = match &self.video_path {
            Some(p) => p.clone(),
            None => return,
        };
        let meta = match &self.video_meta {
            Some(m) => m.clone(),
            None => return,
        };

        // Persiste state antes de rodar
        self.save_state();

        // Clone all slate fields for the thread
        let titulo = self.titulo.clone();
        let produto = self.produto.clone();
        let duracao = self.duracao.clone();
        let produtora = self.produtora.clone();
        let agencia = self.agencia.clone();
        let anunciante = self.anunciante.clone();
        let diretor = self.diretor.clone();
        let registro = self.registro.clone();
        let data = self.data.clone();
        let output_dir = PathBuf::from(&self.output_dir);
        // Se vai enviar depois, força MXF (precisa do arquivo)
        let render_mxf = if then_upload { true } else { self.render_mxf };
        let render_mp4 = self.render_mp4;
        // Se for "encodar e enviar" e não optar por manter, vai apagar o MXF depois
        let keep_mxf = if then_upload {
            self.keep_mxf_after_send
        } else {
            true // só encode → sempre mantém
        };

        // Captura tudo necessário pro upload (pode ser None se não vai enviar)
        let upload_ctx = if then_upload {
            self.build_upload_context()
        } else {
            None
        };

        // Caso "Encodar" sozinho + Compartilhar: captura dados pro Drive standalone
        let drive_only_ctx = if !then_upload && self.share_to_drive {
            self.peach_cfg.as_ref().and_then(|cfg| {
                if cfg.webhook_url.is_empty() || cfg.drive_folder_id.is_empty() {
                    None
                } else {
                    Some((cfg.webhook_url.clone(), cfg.drive_folder_id.clone()))
                }
            })
        } else {
            None
        };

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.encoding = true;
        self.upload_progress = None;
        self.status_text = "Encodando...".to_string();
        self.result_message = None;

        let ctx = ctx.clone();
        let tx_thread = tx.clone();
        let log_tx = make_log_forwarder(tx_thread.clone(), ctx.clone());

        std::thread::spawn(move || {
            // Configura o log thread-local desta worker thread
            encoder_lib::log::set_sender(Some(log_tx));

            let result = run_encode(
                &video_path,
                &meta,
                &titulo,
                &produto,
                &duracao,
                &produtora,
                &agencia,
                &anunciante,
                &diretor,
                &registro,
                &data,
                &output_dir,
                render_mxf,
                render_mp4,
            );

            let mxf_path = output_dir.join(format!("{titulo}.mxf"));

            match result {
                Ok(encode_result) => {
                    if let Some(uctx) = upload_ctx {
                        // Encoding deu certo, agora upload do MXF
                        let _ = tx_thread.send(EncoderMessage::Status(format!(
                            "Encoding OK. Enviando {}...",
                            mxf_path.file_name().unwrap_or_default().to_string_lossy()
                        )));
                        ctx.request_repaint();

                        let upload_result =
                            run_upload(&mxf_path, &titulo, &uctx, tx_thread.clone(), &ctx);

                        match upload_result {
                            Ok(upload_msg) => {
                                // Sucesso total: respeita o checkbox keep_mxf
                                if !keep_mxf {
                                    cleanup_mxf(&mxf_path);
                                }
                                let extra = if !keep_mxf {
                                    "\nMXF removido após envio."
                                } else {
                                    ""
                                };
                                let _ = tx_thread.send(EncoderMessage::Finished(format!(
                                    "{encode_result}\n\n{upload_msg}{extra}"
                                )));
                            }
                            Err(e) => {
                                // Erro no upload/distribuição: MANTÉM o MXF
                                // pra você poder tentar enviar de novo depois sem re-encodar.
                                let _ = tx_thread.send(EncoderMessage::Error(format!(
                                    "Encoding OK, mas upload falhou: {e}\nMXF preservado em {} pra retry.",
                                    mxf_path.display()
                                )));
                            }
                        }
                    } else {
                        // Só encode, sem upload — sempre mantém MXF.
                        // Se checkbox Compartilhar estiver marcado, sobe MP4 pro Drive.
                        let mut final_msg = encode_result;
                        if let Some((webhook_url, folder_id)) = drive_only_ctx {
                            let mp4_path = output_dir
                                .join("agencia")
                                .join(format!("{}.mp4", titulo));
                            match run_drive_only(&mp4_path, &webhook_url, &folder_id, &tx_thread, &ctx) {
                                Ok(url) => {
                                    final_msg = format!("{final_msg}\n\nDrive: {url}");
                                }
                                Err(e) => {
                                    final_msg = format!("{final_msg}\n\n[drive] Falhou: {e}");
                                }
                            }
                        }
                        let _ = tx_thread.send(EncoderMessage::Finished(final_msg));
                    }
                }
                Err(e) => {
                    // Erro no encode. Cleanup do MXF parcial se existir e !keep_mxf
                    if !keep_mxf && mxf_path.exists() {
                        cleanup_mxf(&mxf_path);
                    }
                    let _ = tx_thread.send(EncoderMessage::Error(format!("{e}")));
                }
            }
            ctx.request_repaint();
        });
    }

    /// Inicia apenas o upload de um vídeo já existente (sem encodar).
    /// Procura o MXF: se o selecionado é .mxf usa ele, senão `output_dir/<titulo>.mxf`.
    fn start_send_only(&mut self, ctx: &egui::Context) {
        let mxf_path = match self.find_mxf_for_send() {
            Some(p) => p,
            None => {
                self.result_message = Some((
                    false,
                    "MXF não encontrado. Encode primeiro ou selecione um arquivo .mxf.".into(),
                ));
                return;
            }
        };
        if self.video_meta.is_none() {
            return;
        }

        // Persiste state antes
        self.save_state();

        let titulo = self.titulo.clone();
        let uctx = match self.build_upload_context() {
            Some(c) => c,
            None => {
                self.result_message = Some((
                    false,
                    "Configuração de Peach incompleta para este cliente.".into(),
                ));
                return;
            }
        };

        let keep_mxf = self.keep_mxf_after_send;

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.encoding = true;
        self.upload_progress = None;
        self.status_text = "Enviando para Peach...".into();
        self.result_message = None;

        let ctx = ctx.clone();
        let tx_thread = tx.clone();
        let log_tx = make_log_forwarder(tx_thread.clone(), ctx.clone());

        std::thread::spawn(move || {
            encoder_lib::log::set_sender(Some(log_tx));

            let result = run_upload(&mxf_path, &titulo, &uctx, tx_thread.clone(), &ctx);

            match result {
                Ok(msg) => {
                    if !keep_mxf {
                        cleanup_mxf(&mxf_path);
                    }
                    let extra = if !keep_mxf {
                        "\nMXF removido após envio."
                    } else {
                        ""
                    };
                    let _ = tx_thread.send(EncoderMessage::Finished(format!("{msg}{extra}")));
                }
                Err(e) => {
                    // Mantém o MXF em caso de erro (você pode tentar enviar de novo)
                    let _ = tx_thread.send(EncoderMessage::Error(format!(
                        "{e}\nMXF preservado em {} pra retry.",
                        mxf_path.display()
                    )));
                }
            }
            ctx.request_repaint();
        });
    }

    /// Constrói o contexto necessário para fazer upload (credenciais + config + código).
    fn build_upload_context(&self) -> Option<UploadContext> {
        let cfg = self.peach_cfg.clone()?;
        let creds = peach::PeachCredentials::load(&self.config_dir).ok()?;
        let filename = self.video_path.as_ref()?.file_name()?.to_str()?.to_string();
        let codigo = peach::resolve_codigo_from_filename(&filename, &self.codes)?;

        // Filtra destinos do cfg pelos selecionados na GUI (apenas IDs)
        let destinos_hd: Vec<String> = cfg
            .destinos
            .hd
            .iter()
            .filter(|d| self.selected_destinos.contains(d.id()))
            .map(|d| d.id().to_string())
            .collect();
        let destinos_sd: Vec<String> = cfg
            .destinos
            .sd
            .iter()
            .filter(|d| self.selected_destinos.contains(d.id()))
            .map(|d| d.id().to_string())
            .collect();

        Some(UploadContext {
            cfg,
            creds,
            codigo,
            distribute: self.distribute_after_upload,
            share_to_drive: self.share_to_drive,
            destinos_hd,
            destinos_sd,
        })
    }
}

#[derive(Clone)]
struct UploadContext {
    cfg: peach::PeachConfig,
    creds: peach::PeachCredentials,
    codigo: String,
    /// Se true, após upload S3 distribui automaticamente para os destinos selecionados.
    distribute: bool,
    /// Se true, zipa o MP4 agência e faz upload pro Google Drive (em paralelo com QC).
    share_to_drive: bool,
    /// IDs HD a distribuir (já filtrados pela seleção da GUI).
    destinos_hd: Vec<String>,
    /// IDs SD a distribuir.
    destinos_sd: Vec<String>,
}

/// Remove o MXF após envio (ou erro). Best-effort, ignora falhas.
fn cleanup_mxf(mxf_path: &Path) {
    if mxf_path.exists() {
        match std::fs::remove_file(mxf_path) {
            Ok(_) => encoder_lib::log::emit(format!("[cleanup] MXF removido: {}", mxf_path.display())),
            Err(e) => encoder_lib::log::emit(format!(
                "[cleanup] Falha ao remover {}: {e}",
                mxf_path.display()
            )),
        }
    }
}

/// Cria um sender de log que encaminha cada linha como `EncoderMessage::Log`
/// pro canal principal da GUI. Devolve o sender pra ser passado pro
/// `encoder_lib::log::set_sender` na thread de trabalho.
fn make_log_forwarder(
    tx: mpsc::Sender<EncoderMessage>,
    ctx: egui::Context,
) -> mpsc::Sender<String> {
    let (log_tx, log_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        while let Ok(line) = log_rx.recv() {
            if tx.send(EncoderMessage::Log(line)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    log_tx
}

/// Executa o upload do arquivo (já encodado) para o Peach.
/// Roda numa thread separada e cria seu próprio runtime tokio.
///
/// Re-proba o MXF pra ter os valores reais (fps de saída, duração total)
/// e desconta a claquete (`SLATE_BLACK_TOTAL_SECS`) pra obter a duração comercial
/// — que é o que o Peach espera no campo `segundos`.
fn run_upload(
    mxf_path: &Path,
    titulo: &str,
    uctx: &UploadContext,
    tx: mpsc::Sender<EncoderMessage>,
    ctx: &egui::Context,
) -> anyhow::Result<String> {
    encoder_lib::log::emit(format!("[peach] Iniciando upload de {}", mxf_path.display()));
    if !mxf_path.exists() {
        anyhow::bail!("Arquivo não existe: {}", mxf_path.display());
    }
    let file_size = std::fs::metadata(mxf_path)?.len();
    encoder_lib::log::emit(format!(
        "[peach] Tamanho: {} bytes ({} MB)",
        file_size,
        file_size / 1_048_576
    ));

    // Re-proba o MXF (não usa meta do source) pra ter fps e duração reais do arquivo enviado.
    let mxf_meta = metadata::probe(mxf_path).context("falha ao probar MXF antes do upload")?;
    let framerate = format!("{:.2}", mxf_meta.fps_num as f64 / mxf_meta.fps_den as f64);
    let commercial_secs = mxf_meta
        .duration_secs
        .saturating_sub(encoder::SLATE_BLACK_TOTAL_SECS);
    encoder_lib::log::emit(format!(
        "[peach] MXF: total={}s, comercial={}s (-{}s claquete), fps={}",
        mxf_meta.duration_secs, commercial_secs, encoder::SLATE_BLACK_TOTAL_SECS, framerate
    ));

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        encoder_lib::log::emit(format!("[peach] Login como {}...", uctx.creds.email));
        let _ = tx.send(EncoderMessage::Status("Logando no Peach...".into()));
        ctx.request_repaint();

        let client = peach::PeachClient::new()?;
        let session = client
            .login(&uctx.creds.email, &uctx.creds.password)
            .await?;
        encoder_lib::log::emit(format!(
            "[peach] Login OK: {} ({})",
            session.nombre_usuario_activo, session.id_empresa
        ));

        let params = peach::UploadParams {
            video_path: mxf_path,
            pieza: titulo,
            codigo: &uctx.codigo,
            framerate: &framerate,
            duration_secs: commercial_secs,
        };

        encoder_lib::log::emit(format!(
            "[peach] init_upload: pieza={} codigo={} fps={} dur={}s",
            titulo, uctx.codigo, framerate, commercial_secs
        ));
        let _ = tx.send(EncoderMessage::Status("Obtendo credenciais STS...".into()));
        ctx.request_repaint();

        let sts = client
            .init_upload(&params, &uctx.cfg, &uctx.creds.productora_id)
            .await?;
        encoder_lib::log::emit(format!(
            "[peach] STS OK: id_envio={} bucket={} key={}",
            sts.id_envio, sts.bucket, sts.destination
        ));

        let _ = tx.send(EncoderMessage::Status(format!(
            "Upload S3 ({} MB)...",
            file_size / 1_048_576
        )));
        ctx.request_repaint();

        encoder_lib::log::emit("[peach] Iniciando S3 multipart upload...");
        let tx_progress = tx.clone();
        let ctx_progress = ctx.clone();
        peach::upload::s3_multipart_upload(mxf_path, &sts, move |sent, total| {
            let pct = sent * 100 / total;
            encoder_lib::log::emit(format!("[peach] {}/{} bytes ({}%)", sent, total, pct));
            let _ = tx_progress.send(EncoderMessage::UploadProgress(sent, total));
            ctx_progress.request_repaint();
        })
        .await?;

        encoder_lib::log::emit(format!(
            "[peach] Upload concluído. id_envio={}",
            sts.id_envio
        ));

        let mut summary = format!("Upload Peach OK | id_envio={}", sts.id_envio);

        // --- Drive upload em paralelo (se habilitado) ---
        let mp4_agencia_path = mxf_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("agencia")
            .join(format!("{}.mp4", titulo));

        let drive_future = if uctx.share_to_drive
            && !uctx.cfg.webhook_url.is_empty()
            && !uctx.cfg.drive_folder_id.is_empty()
            && mp4_agencia_path.exists()
        {
            let webhook = uctx.cfg.webhook_url.clone();
            let folder = uctx.cfg.drive_folder_id.clone();
            let path = mp4_agencia_path.clone();
            encoder_lib::log::emit(
                "[drive] Iniciando upload do MP4 agência em paralelo com QC...",
            );
            Some(tokio::spawn(async move {
                peach::drive::upload_mp4_zipped(&webhook, &path, &folder).await
            }))
        } else {
            if uctx.share_to_drive {
                encoder_lib::log::emit(format!(
                    "[drive] Compartilhamento habilitado mas faltam prerrequisitos: \
                     webhook_url={} folder_id={} mp4_existe={}",
                    !uctx.cfg.webhook_url.is_empty(),
                    !uctx.cfg.drive_folder_id.is_empty(),
                    mp4_agencia_path.exists()
                ));
            }
            None
        };

        // --- Distribuição para emissoras ---
        // Info do envio (se distribuiu) pra depois gerar o log com a URL do Drive
        let mut send_info: Option<(u64, Vec<String>)> = None;

        if uctx.distribute {
            if uctx.destinos_hd.is_empty() && uctx.destinos_sd.is_empty() {
                encoder_lib::log::emit(
                    "[peach] Distribuição habilitada mas nenhum destino selecionado — pulando.",
                );
            } else {
                let spot_id = sts.spot_id().ok_or_else(|| {
                    anyhow::anyhow!(
                        "Não foi possível extrair spot_id do destination '{}'",
                        sts.destination
                    )
                })?;

                let _ = tx.send(EncoderMessage::Status(format!(
                    "Distribuindo spot {spot_id} para {} emissoras...",
                    uctx.destinos_hd.len() + uctx.destinos_sd.len()
                )));
                ctx.request_repaint();

                encoder_lib::log::emit(format!(
                    "[peach] Distribuindo spot_id={spot_id} → HD={:?} SD={:?}",
                    uctx.destinos_hd, uctx.destinos_sd
                ));

                let req = peach::SendRequest {
                    spot_ids: &[spot_id],
                    destinos_hd: &uctx.destinos_hd,
                    destinos_sd: &uctx.destinos_sd,
                };

                let send_summary = client.send_spots(&req, &uctx.cfg).await?;
                summary = format!("{summary}\n{send_summary}");

                let destinos_labels: Vec<String> = uctx
                    .cfg
                    .destinos
                    .hd
                    .iter()
                    .chain(uctx.cfg.destinos.sd.iter())
                    .filter(|d| {
                        uctx.destinos_hd.contains(&d.id().to_string())
                            || uctx.destinos_sd.contains(&d.id().to_string())
                    })
                    .map(|d| d.label().to_string())
                    .collect();
                send_info = Some((spot_id, destinos_labels));
            }
        }

        // --- Aguarda Drive (único consumo do drive_future) ---
        let mut agencia_url = String::new();
        if let Some(handle) = drive_future {
            agencia_url = await_drive(handle).await;
            if !agencia_url.is_empty() {
                summary = format!("{summary}\nDrive: {agencia_url}");
            }
        }

        // --- Log CSV + webhook (apenas se distribuiu) ---
        if let Some((spot_id, destinos_labels)) = send_info {
            let log_entry = peach::send::SendLogEntry {
                timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                pieza: titulo.to_string(),
                codigo: uctx.codigo.clone(),
                spot_id,
                destinos: destinos_labels.join("; "),
                id_envio: sts.id_envio.clone(),
                agencia_url: agencia_url.clone(),
            };
            let output_dir = mxf_path.parent().unwrap_or(std::path::Path::new("."));
            if let Err(e) = peach::send::append_send_log(output_dir, &log_entry) {
                encoder_lib::log::emit(format!("[peach] Aviso: falha ao gravar log CSV: {e}"));
            }

            let client_name = uctx.cfg.avisador_id.clone();
            peach::send::post_webhook(&uctx.cfg.webhook_url, &log_entry, &client_name).await;
        }

        Ok(summary)
    })
}

/// Sobe o MP4 agência pro Google Drive (standalone, sem passar pelo Peach).
/// Usado quando o botão "Encodar" é clicado com o checkbox "Compartilhar MP4" marcado.
/// Roda num runtime tokio próprio (thread sync).
fn run_drive_only(
    mp4_path: &Path,
    webhook_url: &str,
    folder_id: &str,
    tx: &mpsc::Sender<EncoderMessage>,
    ctx: &egui::Context,
) -> anyhow::Result<String> {
    if !mp4_path.exists() {
        anyhow::bail!(
            "MP4 agência não existe em {}. Verifique se o checkbox MP4 (agência) está marcado.",
            mp4_path.display()
        );
    }
    let _ = tx.send(EncoderMessage::Status(
        "Compartilhando MP4 no Drive...".into(),
    ));
    ctx.request_repaint();

    let rt = tokio::runtime::Runtime::new()?;
    let result =
        rt.block_on(async { peach::drive::upload_mp4_zipped(webhook_url, mp4_path, folder_id).await })?;
    Ok(result.url)
}

/// Aguarda um JoinHandle de upload no Drive e retorna a URL (ou string vazia em erro).
async fn await_drive(
    handle: tokio::task::JoinHandle<anyhow::Result<peach::DriveUploadResult>>,
) -> String {
    match handle.await {
        Ok(Ok(result)) => {
            encoder_lib::log::emit(format!("[drive] URL: {}", result.url));
            result.url
        }
        Ok(Err(e)) => {
            encoder_lib::log::emit(format!("[drive] Upload falhou: {e}"));
            String::new()
        }
        Err(e) => {
            encoder_lib::log::emit(format!("[drive] Task panicou: {e}"));
            String::new()
        }
    }
}

fn run_encode(
    video_path: &Path,
    meta: &metadata::VideoMetadata,
    titulo: &str,
    produto: &str,
    duracao: &str,
    produtora: &str,
    agencia: &str,
    anunciante: &str,
    diretor: &str,
    registro: &str,
    data: &str,
    output_dir: &Path,
    render_mxf: bool,
    render_mp4: bool,
) -> anyhow::Result<String> {
    // Create output dir
    std::fs::create_dir_all(output_dir)?;

    let mut results = Vec::new();

    // Encode MXF (com claquete)
    if render_mxf {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));

        let template_path = encoder_lib::find_template(&exe_dir)?;
        let temp_slate = std::env::temp_dir().join("encoder_temp_slate.png");

        let defaults = config::Defaults {
            produto: produto.to_string(),
            produtora: produtora.to_string(),
            agencia: agencia.to_string(),
            anunciante: anunciante.to_string(),
            diretor: diretor.to_string(),
            output: String::new(),
        };

        let slate_data = slate::SlateData::new(titulo, duracao, registro, data, &defaults);
        slate::generate_slate(&template_path, &slate_data, &temp_slate)?;

        let output_path = output_dir.join(format!("{titulo}.mxf"));
        encoder::encode(&temp_slate, video_path, &output_path, meta)?;

        let _ = std::fs::remove_file(&temp_slate);
        results.push(format!("MXF: {}", output_path.display()));
    }

    // Encode MP4 agência (sem claquete)
    if render_mp4 {
        let agency_dir = output_dir.join("agencia");
        std::fs::create_dir_all(&agency_dir)?;
        let agency_path = agency_dir.join(format!("{titulo}.mp4"));
        encoder::encode_agency(video_path, &agency_path, meta)?;
        results.push(format!("Agência: {}", agency_path.display()));
    }

    Ok(results.join("\n"))
}

impl eframe::App for EncoderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check for messages from background thread (drain todas)
        if self.rx.is_some() {
            // Coleta todas as mensagens prontas em um vec local pra evitar borrow conflict
            let msgs: Vec<EncoderMessage> = {
                let rx = self.rx.as_ref().unwrap();
                let mut v = Vec::new();
                while let Ok(m) = rx.try_recv() {
                    v.push(m);
                }
                v
            };
            for msg in msgs {
                match msg {
                    EncoderMessage::Log(line) => {
                        self.push_log(line);
                    }
                    EncoderMessage::Status(s) => {
                        self.push_log(format!("[status] {s}"));
                        self.status_text = s;
                    }
                    EncoderMessage::UploadProgress(sent, total) => {
                        self.upload_progress = Some((sent, total));
                    }
                    EncoderMessage::Finished(path) => {
                        self.push_log(format!("[OK] {}", path.replace('\n', " | ")));
                        self.result_message = Some((true, format!("Concluído:\n{path}")));
                        self.encoding = false;
                        self.upload_progress = None;
                        self.status_text.clear();
                        self.rx = None;
                    }
                    EncoderMessage::Error(err) => {
                        self.push_log(format!("[ERRO] {err}"));
                        self.result_message = Some((false, format!("Erro: {err}")));
                        self.encoding = false;
                        self.upload_progress = None;
                        self.status_text.clear();
                        self.rx = None;
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Encoder - Claquete + MXF XDCAM HD422");
            ui.add_space(8.0);

            // --- Client selector (only if clients exist) ---
            if !self.available_clients.is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Cliente:");
                    let current_label = self
                        .selected_client
                        .as_deref()
                        .unwrap_or("(Padrão)");
                    egui::ComboBox::from_id_salt("client_selector")
                        .selected_text(current_label)
                        .show_ui(ui, |ui| {
                            let mut changed = false;
                            if ui
                                .selectable_value(&mut self.selected_client, None, "(Padrão)")
                                .changed()
                            {
                                changed = true;
                            }
                            for client in self.available_clients.clone() {
                                if ui
                                    .selectable_value(
                                        &mut self.selected_client,
                                        Some(client.clone()),
                                        &client,
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                            if changed {
                                self.reload_config();
                                self.save_state();
                            }
                        });
                });
                ui.add_space(4.0);
            }

            // Config error banner
            if let Some(err) = &self.config_error {
                ui.colored_label(egui::Color32::RED, err);
                ui.add_space(4.0);
            }

            // --- Video selection ---
            ui.horizontal(|ui| {
                if ui.button("Selecionar Vídeo").clicked() {
                    self.select_video();
                }
                if let Some(path) = &self.video_path {
                    ui.monospace(path.display().to_string());
                }
            });

            if let Some(err) = &self.probe_error {
                ui.colored_label(egui::Color32::RED, err);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // --- Slate fields ---
            let field_width = 400.0;

            egui::Grid::new("slate_fields")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Título:");
                    ui.add(egui::TextEdit::singleline(&mut self.titulo).desired_width(field_width));
                    ui.end_row();

                    ui.label("Produto:");
                    ui.add(egui::TextEdit::singleline(&mut self.produto).desired_width(field_width));
                    ui.end_row();

                    ui.label("Duração:");
                    ui.add(egui::TextEdit::singleline(&mut self.duracao).desired_width(field_width));
                    ui.end_row();

                    ui.label("Produtora:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.produtora)
                            .desired_width(field_width),
                    );
                    ui.end_row();

                    ui.label("Agência:");
                    ui.add(egui::TextEdit::singleline(&mut self.agencia).desired_width(field_width));
                    ui.end_row();

                    ui.label("Anunciante:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.anunciante)
                            .desired_width(field_width),
                    );
                    ui.end_row();

                    ui.label("Diretor:");
                    ui.add(egui::TextEdit::singleline(&mut self.diretor).desired_width(field_width));
                    ui.end_row();

                    ui.label("Registro:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.registro).desired_width(field_width),
                    );
                    ui.end_row();

                    ui.label("Data:");
                    ui.add(egui::TextEdit::singleline(&mut self.data).desired_width(field_width));
                    ui.end_row();
                });

            // Registro warning
            if let Some(warn) = &self.registro_warning {
                ui.add_space(2.0);
                ui.colored_label(egui::Color32::YELLOW, warn);
            }

            ui.add_space(8.0);

            // --- Output dir ---
            ui.horizontal(|ui| {
                ui.label("Output:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.output_dir).desired_width(field_width),
                );
                if ui.button("Procurar").clicked() {
                    if let Some(dir) = rfd::FileDialog::new()
                        .set_title("Selecionar pasta de saída")
                        .pick_folder()
                    {
                        self.output_dir = dir.display().to_string();
                    }
                }
            });

            ui.add_space(8.0);

            // --- Video metadata display ---
            if let Some(meta) = &self.video_meta {
                ui.horizontal(|ui| {
                    ui.label("Metadados:");
                    ui.monospace(format!(
                        "{}x{} | {}/{} fps | {}s | {}",
                        meta.width,
                        meta.height,
                        meta.fps_num,
                        meta.fps_den,
                        meta.duration_secs,
                        if meta.has_audio {
                            format!("{} canais", meta.audio_channels)
                        } else {
                            "sem áudio".to_string()
                        }
                    ));
                });

                ui.add_space(8.0);
            }

            ui.separator();
            ui.add_space(8.0);

            // --- Render options ---
            ui.horizontal(|ui| {
                ui.label("Formatos:");
                ui.checkbox(&mut self.render_mxf, "MXF (claquete + XDCAM)");
                ui.checkbox(&mut self.render_mp4, "MP4 (agência)");
            });
            ui.horizontal(|ui| {
                ui.label("Envio:");
                ui.checkbox(&mut self.keep_mxf_after_send, "Manter MXF após o envio")
                    .on_hover_text(
                        "Se desmarcado, o MXF é removido após o upload (mesmo em caso de erro).\nNão se aplica a 'Encodar' sozinho.",
                    );
                let has_destinos = self
                    .peach_cfg
                    .as_ref()
                    .map(|c| !c.destinos.is_empty())
                    .unwrap_or(false);
                ui.add_enabled(
                    has_destinos,
                    egui::Checkbox::new(
                        &mut self.distribute_after_upload,
                        "Distribuir para emissoras",
                    ),
                )
                .on_hover_text(
                    "Após o upload, distribui automaticamente para os destinos selecionados.",
                )
                .on_disabled_hover_text(
                    "Cliente atual não tem [peach.destinos] configurado em defaults.toml.",
                );

                let drive_ready = self
                    .peach_cfg
                    .as_ref()
                    .map(|c| !c.webhook_url.is_empty() && !c.drive_folder_id.is_empty())
                    .unwrap_or(false);
                ui.add_enabled(
                    drive_ready,
                    egui::Checkbox::new(&mut self.share_to_drive, "Compartilhar MP4 (Drive)"),
                )
                .on_hover_text(
                    "Zipa o MP4 agência e envia pro Google Drive em paralelo com o QC.\nRetorna URL compartilhável (sobrescreve se já existe arquivo com mesmo nome).",
                )
                .on_disabled_hover_text(
                    "Precisa configurar webhook_url e drive_folder_id no [peach] do cliente.",
                );
            });

            // --- Lista de destinos (checkboxes por emissora) ---
            if let Some(cfg) = self.peach_cfg.clone() {
                if !cfg.destinos.is_empty() && self.distribute_after_upload {
                    ui.add_space(2.0);
                    ui.indent("destinos_indent", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Emissoras:");
                            if ui.small_button("Marcar todos").clicked() {
                                for d in cfg.destinos.hd.iter().chain(cfg.destinos.sd.iter()) {
                                    self.selected_destinos.insert(d.id().to_string());
                                }
                                self.save_state();
                            }
                            if ui.small_button("Desmarcar todos").clicked() {
                                self.selected_destinos.clear();
                                self.save_state();
                            }
                        });
                        let mut changed = false;
                        for d in &cfg.destinos.hd {
                            let id = d.id().to_string();
                            let mut sel = self.selected_destinos.contains(&id);
                            if ui
                                .checkbox(&mut sel, format!("HD  {}  ({})", d.label(), id))
                                .changed()
                            {
                                if sel {
                                    self.selected_destinos.insert(id);
                                } else {
                                    self.selected_destinos.remove(&id);
                                }
                                changed = true;
                            }
                        }
                        for d in &cfg.destinos.sd {
                            let id = d.id().to_string();
                            let mut sel = self.selected_destinos.contains(&id);
                            if ui
                                .checkbox(&mut sel, format!("SD  {}  ({})", d.label(), id))
                                .changed()
                            {
                                if sel {
                                    self.selected_destinos.insert(id);
                                } else {
                                    self.selected_destinos.remove(&id);
                                }
                                changed = true;
                            }
                        }
                        if changed {
                            self.save_state();
                        }
                    });
                }
            }

            ui.add_space(8.0);

            // --- Action buttons ---
            let has_video = self.video_path.is_some() && self.video_meta.is_some();
            let any_format = self.render_mxf || self.render_mp4;
            let peach_ready = self.peach_cfg.is_some() && self.peach_creds_error.is_none();

            ui.horizontal(|ui| {
                // Botão 1: Encodar (respeita checkboxes)
                let can_encode = has_video && !self.encoding && any_format;
                if ui
                    .add_enabled(can_encode, egui::Button::new("Encodar"))
                    .on_disabled_hover_text("Selecione um vídeo e marque ao menos um formato")
                    .clicked()
                {
                    self.start_encoding(ctx, false);
                }

                // Botão 2: Encodar e Enviar (força MXF, depois envia)
                let can_encode_send = has_video && !self.encoding && peach_ready;
                let btn = ui.add_enabled(can_encode_send, egui::Button::new("Encodar e Enviar"));
                let btn = if !peach_ready {
                    btn.on_disabled_hover_text(
                        "Cliente atual não tem [peach] configurado ou faltam credenciais",
                    )
                } else if !has_video {
                    btn.on_disabled_hover_text("Selecione um vídeo")
                } else {
                    btn
                };
                if btn.clicked() {
                    self.start_encoding(ctx, true);
                }

                // Botão 3: Enviar (procura MXF: selecionado direto OU output_dir/<titulo>.mxf)
                let mxf_for_send = self.find_mxf_for_send();
                let can_send =
                    has_video && !self.encoding && peach_ready && mxf_for_send.is_some();
                let btn = ui.add_enabled(can_send, egui::Button::new("Enviar"));
                let btn = if mxf_for_send.is_none() && has_video {
                    btn.on_disabled_hover_text(
                        "MXF não encontrado. Encode primeiro ou selecione um arquivo .mxf.",
                    )
                } else if !peach_ready {
                    btn.on_disabled_hover_text(
                        "Cliente atual não tem [peach] configurado ou faltam credenciais",
                    )
                } else {
                    btn
                };
                if btn.clicked() {
                    self.start_send_only(ctx);
                }

                if self.encoding {
                    ui.spinner();
                    ui.label(&self.status_text);
                }
            });

            // Status Peach (banner amarelo se faltarem credenciais)
            if let Some(err) = &self.peach_creds_error {
                ui.add_space(2.0);
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Peach: {} (salvar em config/peach_credentials.toml)", err),
                );
            }

            // Progresso do upload
            if let Some((sent, total)) = self.upload_progress {
                let pct = if total > 0 {
                    sent as f32 / total as f32
                } else {
                    0.0
                };
                ui.add_space(4.0);
                ui.add(egui::ProgressBar::new(pct).text(format!(
                    "{:.0}%  ({} / {} MB)",
                    pct * 100.0,
                    sent / 1_048_576,
                    total / 1_048_576
                )));
            }

            ui.add_space(8.0);

            // --- Result ---
            if let Some((success, msg)) = &self.result_message {
                let color = if *success {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::RED
                };
                ui.colored_label(color, msg);
            }

            ui.add_space(8.0);
            ui.separator();

            // --- Log panel ---
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.show_log, "Mostrar log");
                if ui.button("Limpar").clicked() {
                    self.log_lines.clear();
                }
                ui.label(format!("({} linhas)", self.log_lines.len()));
            });

            if self.show_log {
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let text = self
                            .log_lines
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        ui.add(
                            egui::TextEdit::multiline(&mut text.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(10),
                        );
                    });
            }
        });
    }
}

fn find_config_dir() -> PathBuf {
    // Try next to the executable first, then fall back to CWD
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("config");
            if candidate.is_dir() {
                return candidate;
            }
            // Check one level up (for target/release/ layout)
            if let Some(parent) = exe_dir.parent().and_then(|p| p.parent()) {
                let candidate = parent.join("config");
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }
    }
    PathBuf::from("config")
}

fn main() -> eframe::Result {
    // Check ffmpeg upfront
    if let Err(e) = metadata::check_ffmpeg() {
        eprintln!("Aviso: {e}");
    }

    let args = GuiArgs::parse();
    let initial_video = args.video.filter(|p| p.exists());
    let initial_client = args.client;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 900.0])
            .with_min_inner_size([700.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Encoder - Claquete + MXF",
        options,
        Box::new(|_cc| Ok(Box::new(EncoderApp::new(initial_video, initial_client)))),
    )
}
