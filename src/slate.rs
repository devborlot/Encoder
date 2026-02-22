use ab_glyph::{FontRef, PxScale};
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use std::path::Path;

use crate::config::Defaults;

/// Dados dinâmicos para renderizar na claquete
pub struct SlateData<'a> {
    pub titulo: &'a str,
    pub produto: &'a str,
    pub duracao: &'a str,
    pub produtora: &'a str,
    pub agencia: &'a str,
    pub anunciante: &'a str,
    pub diretor: &'a str,
    pub registro: &'a str,
    pub data: &'a str,
}

impl<'a> SlateData<'a> {
    pub fn new(
        titulo: &'a str,
        duracao: &'a str,
        registro: &'a str,
        data: &'a str,
        defaults: &'a Defaults,
    ) -> Self {
        Self {
            titulo,
            produto: &defaults.produto,
            duracao,
            produtora: &defaults.produtora,
            agencia: &defaults.agencia,
            anunciante: &defaults.anunciante,
            diretor: &defaults.diretor,
            registro,
            data,
        }
    }
}

struct FieldPosition {
    x: i32,
    y: i32,
}

/// Posições dos campos na claquete (calibradas conforme o template)
const FIELD_POSITIONS: &[(&str, FieldPosition)] = &[
    ("titulo", FieldPosition { x: 470, y: 198 }),
    ("produto", FieldPosition { x: 470, y: 271 }),
    ("duracao", FieldPosition { x: 470, y: 344 }),
    ("produtora", FieldPosition { x: 470, y: 417 }),
    ("agencia", FieldPosition { x: 470, y: 490 }),
    ("anunciante", FieldPosition { x: 470, y: 563 }),
    ("diretor", FieldPosition { x: 470, y: 636 }),
    ("registro", FieldPosition { x: 470, y: 709 }),
    ("data", FieldPosition { x: 470, y: 782 }),
];

pub fn generate_slate(
    template_path: &Path,
    data: &SlateData,
    output_path: &Path,
) -> Result<()> {
    // Carregar template
    let mut img: RgbaImage = image::open(template_path)
        .with_context(|| format!("Não foi possível abrir template: {}", template_path.display()))?
        .to_rgba8();

    // Carregar fonte Arial Bold do sistema
    let font_path = r"C:\Windows\Fonts\arialbd.ttf";
    let font_bytes =
        std::fs::read(font_path).with_context(|| format!("Fonte não encontrada: {font_path}"))?;
    let font =
        FontRef::try_from_slice(&font_bytes).context("Falha ao carregar fonte Arial Bold")?;

    let scale = PxScale::from(36.0);
    let color = Rgba([0u8, 0, 0, 255]); // Preto

    // Mapear nomes dos campos aos valores
    let fields: Vec<(&str, &str)> = vec![
        ("titulo", data.titulo),
        ("produto", data.produto),
        ("duracao", data.duracao),
        ("produtora", data.produtora),
        ("agencia", data.agencia),
        ("anunciante", data.anunciante),
        ("diretor", data.diretor),
        ("registro", data.registro),
        ("data", data.data),
    ];

    for (field_name, value) in &fields {
        if let Some((_, pos)) = FIELD_POSITIONS.iter().find(|(name, _)| name == field_name) {
            draw_text_mut(&mut img, color, pos.x, pos.y, scale, &font, value);
        }
    }

    img.save(output_path)
        .with_context(|| format!("Falha ao salvar claquete: {}", output_path.display()))?;

    Ok(())
}
