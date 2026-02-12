use axum::{
    extract::{Form, State, Multipart},
    routing::{get, post},
    response::{Html, IntoResponse},
    Json, Router,
};
use serde::Deserialize;
use sqlx::{PgPool, Row};
use std::{env, net::SocketAddr};
use tower_http::{cors::CorsLayer, services::ServeDir};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use regex::Regex;

const MAX_IMAGE_SIZE: usize = 5 * 1024 * 1024;
const ALLOWED_MIME: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/jpg"];

#[derive(Deserialize)]
struct FormData {
    nombre: String,
    mensaje: String,
    #[serde(rename = "g-recaptcha-response")]
    recaptcha: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let pool = PgPool::connect(&env::var("DATABASE_URL").unwrap()).await.unwrap();

    let app = Router::new()
        .route("/enviar", post(enviar))
        .route("/upload-image", post(upload_image))
        .route("/images", get(list_images))
        .nest_service("/uploads", ServeDir::new("uploads"))
        .fallback_service(ServeDir::new("static"))
        .with_state(pool)
        .layer(CorsLayer::permissive());

    let port: u16 = env::var("PORT").unwrap_or("3000".into()).parse().unwrap();
    let addr = SocketAddr::from(([0,0,0,0], port));

    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app).await.unwrap();
}

/* ---------- MENSAJES ---------- */

async fn enviar(
    State(pool): State<PgPool>,
    Form(mut data): Form<FormData>,
) -> impl IntoResponse {

    sanitize_text(&mut data.nombre);
    sanitize_text(&mut data.mensaje);

    let name_re = Regex::new(r"^[a-zA-ZáéíóúÁÉÍÓÚñÑ\s]{3,50}$").unwrap();

    if !name_re.is_match(&data.nombre) {
        return Html("❌ Nombre inválido");
    }

    if data.mensaje.len() < 10 || data.mensaje.len() > 500 {
        return Html("❌ Mensaje inválido");
    }

    if data.recaptcha.is_empty() {
        return Html("❌ Completa el reCAPTCHA");
    }

    match sqlx::query("INSERT INTO mensajes (nombre, mensaje) VALUES ($1,$2)")
        .bind(&data.nombre)
        .bind(&data.mensaje)
        .execute(&pool)
        .await
    {
        Ok(_) => Html("✅ Mensaje enviado correctamente"),
        Err(_) => Html("❌ Error guardando mensaje"),
    }
}

/* ---------- SUBIR IMÁGENES ---------- */

async fn upload_image(
    State(pool): State<PgPool>,
    mut multipart: Multipart,
) -> impl IntoResponse {

    tokio::fs::create_dir_all("uploads").await.unwrap();

    while let Some(mut field) = multipart.next_field().await.unwrap() {

        if field.name() != Some("image") {
            continue;
        }

        // 👇 CLAVE: copiamos el MIME a String
        let mime = field
            .content_type()
            .map(|m| m.to_string())
            .unwrap_or_default();

        if !ALLOWED_MIME.contains(&mime.as_str()) {
            return Html("❌ Tipo de archivo no permitido").into_response();
        }

        let bytes = field.bytes().await.unwrap();

        if bytes.len() > MAX_IMAGE_SIZE {
            return Html("❌ Imagen demasiado grande (máx 5MB)").into_response();
        }

        let extension = match mime.as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/png" => "png",
            "image/webp" => "webp",
            _ => return Html("❌ Formato inválido").into_response(),
        };

        let filename = format!("{}.{}", Uuid::new_v4(), extension);
        let path = format!("uploads/{}", filename);

        let mut file = tokio::fs::File::create(&path).await.unwrap();
        file.write_all(&bytes).await.unwrap();

        sqlx::query("INSERT INTO images (filename) VALUES ($1)")
            .bind(&filename)
            .execute(&pool)
            .await
            .unwrap();
    }

    Html("✅ Imagen subida correctamente").into_response()
}


/* ---------- LISTAR ---------- */

async fn list_images(State(pool): State<PgPool>) -> Json<Vec<String>> {
    let rows = sqlx::query("SELECT filename FROM images ORDER BY created_at DESC")
        .fetch_all(&pool)
        .await
        .unwrap();

    Json(rows.into_iter().map(|r| format!("/uploads/{}", r.get::<String,_>("filename"))).collect())
}

/* ---------- UTIL ---------- */

fn sanitize_text(text: &mut String) {
    let forbidden = ["<", ">", "\"", "'", ";", "--", "script"];
    for f in forbidden {
        *text = text.replace(f, "");
    }
}
