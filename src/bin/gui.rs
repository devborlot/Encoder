use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use encoder_lib::{config, encoder, metadata, slate};

// --- Messages from background thread ---

enum EncoderMessage {
    Finished(String),
    Error(String),
}

// --- App State ---

struct EncoderApp {
    // Config
    codes: HashMap<u32, String>,
    config_error: Option<String>,

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

    // Registro warning
    registro_warning: Option<String>,

    // Encoding state
    encoding: bool,
    result_message: Option<(bool, String)>, // (success, message)
    rx: Option<mpsc::Receiver<EncoderMessage>>,
}

impl EncoderApp {
    fn new(initial_video: Option<PathBuf>) -> Self {
        let config_dir = find_config_dir();
        let mut defaults = None;
        let mut codes = HashMap::new();
        let mut config_error = None;

        match config::load_defaults(&config_dir) {
            Ok(d) => defaults = Some(d),
            Err(e) => config_error = Some(format!("Erro ao carregar defaults.toml: {e}")),
        }

        match config::load_codes(&config_dir) {
            Ok(c) => codes = c,
            Err(e) => {
                let msg = format!("Erro ao carregar codes.toml: {e}");
                config_error = Some(match config_error {
                    Some(prev) => format!("{prev}\n{msg}"),
                    None => msg,
                });
            }
        }

        let ano = chrono::Datelike::year(&chrono::Local::now()).to_string();

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

        let mut app = Self {
            codes,
            config_error,
            video_path: None,
            video_meta: None,
            probe_error: None,
            titulo: String::new(),
            produto,
            duracao: String::new(),
            produtora,
            agencia,
            anunciante,
            diretor,
            registro: String::new(),
            data: ano,
            output_dir: "./output".to_string(),
            registro_warning: None,
            encoding: false,
            result_message: None,
            rx: None,
        };

        if let Some(path) = initial_video {
            app.load_video(path);
        }

        app
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

        // Default output dir next to the video
        if let Some(parent) = path.parent() {
            self.output_dir = parent.join("output").display().to_string();
        }

        self.video_path = Some(path);
    }

    fn start_encoding(&mut self, ctx: &egui::Context) {
        let video_path = match &self.video_path {
            Some(p) => p.clone(),
            None => return,
        };
        let meta = match &self.video_meta {
            Some(m) => m.clone(),
            None => return,
        };

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

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.encoding = true;
        self.result_message = None;

        let ctx = ctx.clone();

        std::thread::spawn(move || {
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
            );

            let msg = match result {
                Ok(output_path) => EncoderMessage::Finished(output_path),
                Err(e) => EncoderMessage::Error(format!("{e}")),
            };

            let _ = tx.send(msg);
            ctx.request_repaint();
        });
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
) -> anyhow::Result<String> {
    // Find template
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let template_path = encoder_lib::find_template(&exe_dir)?;
    let temp_slate = std::env::temp_dir().join("encoder_temp_slate.png");

    // Build defaults struct for SlateData
    let defaults = config::Defaults {
        produto: produto.to_string(),
        produtora: produtora.to_string(),
        agencia: agencia.to_string(),
        anunciante: anunciante.to_string(),
        diretor: diretor.to_string(),
    };

    let slate_data = slate::SlateData::new(titulo, duracao, registro, data, &defaults);
    slate::generate_slate(&template_path, &slate_data, &temp_slate)?;

    // Create output dir
    std::fs::create_dir_all(output_dir)?;

    let output_filename = format!("{titulo}.mxf");
    let output_path = output_dir.join(&output_filename);

    // Encode MXF
    encoder::encode(&temp_slate, video_path, &output_path, meta)?;

    // Encode versão agência (MP4 sem claquete)
    let agency_dir = output_dir.join("agencia");
    std::fs::create_dir_all(&agency_dir)?;
    let agency_path = agency_dir.join(format!("{titulo}.mp4"));
    encoder::encode_agency(video_path, &agency_path, meta)?;

    // Clean up temp
    let _ = std::fs::remove_file(&temp_slate);

    Ok(format!(
        "{}\nAgência: {}",
        output_path.display(),
        agency_path.display()
    ))
}

impl eframe::App for EncoderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check for messages from background thread
        if let Some(rx) = &self.rx {
            if let Ok(msg) = rx.try_recv() {
                match msg {
                    EncoderMessage::Finished(path) => {
                        self.result_message = Some((true, format!("Encoding concluído: {path}")));
                    }
                    EncoderMessage::Error(err) => {
                        self.result_message = Some((false, format!("Erro: {err}")));
                    }
                }
                self.encoding = false;
                self.rx = None;
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Encoder - Claquete + MXF XDCAM HD422");
            ui.add_space(8.0);

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

            // --- Action buttons ---
            ui.horizontal(|ui| {
                let can_encode =
                    self.video_path.is_some() && self.video_meta.is_some() && !self.encoding;

                if ui
                    .add_enabled(can_encode, egui::Button::new("Encodar"))
                    .clicked()
                {
                    self.start_encoding(ctx);
                }

                ui.add_enabled(false, egui::Button::new("Encodar e Enviar (em breve)"));

                if self.encoding {
                    ui.spinner();
                    ui.label("Encodando...");
                }
            });

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

    // Accept optional video path as first argument (for context menu integration)
    let initial_video = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.exists());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([700.0, 620.0])
            .with_min_inner_size([600.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Encoder - Claquete + MXF",
        options,
        Box::new(|_cc| Ok(Box::new(EncoderApp::new(initial_video)))),
    )
}
