use std::sync::OnceLock;

use anyhow::{Context as _, Result};
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router, Server,
};
use mathjax_svg::convert_to_svg;
use resvg::usvg::{self, fontdb::Database, Tree};
use serde::Deserialize;
use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};
use usvg::{fontdb, TreeParsing, TreeTextToPath};

#[tokio::main]
async fn main() -> Result<()> {
    let app = Router::new()
        .route("/render/svg", post(svg_handler))
        .route("/render/png", post(png_handler));
    Server::bind(&ADDR.parse()?)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

/// Address to bind
const ADDR: &str = "0.0.0.0:3000";
/// The height of the PNG
const HEIGHT: u32 = 100;
/// Padding size
const PADDING: u32 = 20;
/// Default font-family for <text> tag
#[cfg(target_os = "macos")]
const FONT_FAMILY: &str = "Hiragino Mincho ProN";
#[cfg(target_os = "windows")]
const FONT_FAMILY: &str = "Yu Mincho";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const FONT_FAMILY: &str = "Noto Serif CJK JP";

/// Error (to be resolved during execution)
#[derive(thiserror::Error, Debug)]
enum Error {
    #[error(transparent)]
    LaTeX(#[from] mathjax_svg::Error),
    #[error(transparent)]
    Png(#[from] png::EncodingError),
    #[error(transparent)]
    Svg(#[from] usvg::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let msg = self.to_string();
        if let Error::LaTeX(_) = &self {
            let mut out = String::from("LaTeX Error: ");
            out.push_str(&msg);
            (StatusCode::BAD_REQUEST, out).into_response()
        } else {
            let mut out = String::from("Internal Error: ");
            out.push_str(&msg);
            (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
        }
    }
}

/// Schema of the requests
#[derive(Deserialize, Debug)]
struct Request {
    latex: String,
}

/// Create HeaderMap from content_type &'static str
macro_rules! headers_from_content_type {
    ($content_type: literal) => {{
        static CONTENT_TYPE: HeaderValue = HeaderValue::from_static($content_type as &'static str);
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, CONTENT_TYPE.clone());
        headers
    }};
}

/// Handler to convert math to SVG
async fn svg_handler(Json(req): Json<Request>) -> Result<impl IntoResponse, Error> {
    let svg = convert_to_svg(req.latex)?;
    Ok((headers_from_content_type!("image/svg+xml"), svg))
}

/// Font database: only needs to be initialized once
static FONTDB: OnceLock<Database> = OnceLock::new();

/// Handler to convert math to PNG
async fn png_handler(Json(req): Json<Request>) -> Result<impl IntoResponse, Error> {
    let svg = convert_to_svg(req.latex)?;
    let png = {
        let image = {
            // Convert to Pixmap
            let svg_data = svg.into_bytes();
            let rtree = {
                let opt = usvg::Options::default();

                let mut tree = Tree::from_data(&svg_data, &opt)?;
                tree.convert_text(FONTDB.get_or_init(|| {
                    let mut fdb = fontdb::Database::new();
                    fdb.load_system_fonts();
                    // Set default serif font
                    fdb.set_serif_family(FONT_FAMILY);
                    fdb
                }));
                resvg::Tree::from_usvg(&tree)
            };

            // Vertical length is scaled to be HEIGHT
            let (mut math_pix, scale_x, scale_y) = {
                let original_size = rtree.size;
                let target_size = original_size
                    .to_int_size()
                    .scale_to_height(HEIGHT)
                    .context("scaling Pixmap")?;
                (
                    tiny_skia::Pixmap::new(target_size.width(), target_size.height())
                        .context("creating new Pixmap to draw svg in")?,
                    target_size.width() as f32 / original_size.width(),
                    target_size.height() as f32 / original_size.height(),
                )
            };
            rtree.render(
                tiny_skia::Transform::from_scale(scale_x, scale_y),
                &mut math_pix.as_mut(),
            );
            math_pix
        };

        let image = {
            // Add padding and white background
            let mut background =
                Pixmap::new(PADDING * 2 + image.width(), PADDING * 2 + image.height())
                    .context("creating new Pixmap for padding")?;
            background.fill(Color::WHITE);
            background.draw_pixmap(
                PADDING as i32,
                PADDING as i32,
                image.as_ref(),
                &PixmapPaint::default(),
                Transform::default(),
                None,
            );
            background
        };

        image.encode_png()?
    };
    Ok((headers_from_content_type!("image/png"), png))
}
