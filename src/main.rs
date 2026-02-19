use axum::{
    extract::{Form, State, Multipart, Path},
    routing::{get, post, delete, put},
    response::{Html, IntoResponse},
    Json, Router,
};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
struct Mensaje {
    id: i32,
    nombre: String,
    mensaje: String,
}

#[derive(Deserialize)]
struct UpdateData {
    nombre: String,
    mensaje: String,
}

#[derive(Serialize)]
struct Image {
    id: i32,
    filename: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let pool = PgPool::connect(&env::var("DATABASE_URL").unwrap()).await.unwrap();

    let app = Router::new()
        // RUTAS PÚBLICAS
        .route("/enviar", post(enviar))
        .route("/upload-image", post(upload_image))
        .route("/images", get(list_images))
        
        // ACCESO AL PANEL ADMIN (Limpio)
        .route("/admin", get(|| async { Html(include_str!("../static/admin_mensajes.html")) }))

        // CRUD MENSAJES
        .route("/mensajes", get(list_mensajes))
        .route("/mensajes/:id", delete(delete_mensaje))
        .route("/mensajes/:id", put(update_mensaje))

        // CRUD GALERÍA (Para borrar fotos desde el admin en el futuro)
        .route("/images/:filename", delete(eliminar_imagen_fisica))

        // SERVICIOS
        .nest_service("/uploads", ServeDir::new("uploads"))
        .fallback_service(ServeDir::new("static"))
        .with_state(pool)
        .layer(CorsLayer::permissive());

    let port: u16 = env::var("PORT").unwrap_or("3000".into()).parse().unwrap();
    let addr = SocketAddr::from(([0,0,0,0], port));

    println!("🚀 Servidor corriendo en http://localhost:{}", port);
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
    if !name_re.is_match(&data.nombre) { return Html("❌ Nombre inválido"); }
    if data.mensaje.len() < 10 || data.mensaje.len() > 500 { return Html("❌ Mensaje inválido"); }
    if data.recaptcha.is_empty() { return Html("❌ Completa el reCAPTCHA"); }

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

async fn list_mensajes(State(pool): State<PgPool>) -> Json<Vec<Mensaje>> {
    let rows = sqlx::query("SELECT id, nombre, mensaje FROM mensajes ORDER BY id DESC")
        .fetch_all(&pool)
        .await
        .unwrap();

    let data = rows.into_iter().map(|r| Mensaje {
        id: r.get("id"),
        nombre: r.get("nombre"),
        mensaje: r.get("mensaje"),
    }).collect();

    Json(data)
}

async fn update_mensaje(
    State(pool): State<PgPool>,
    Path(id): Path<i32>,
    Form(mut data): Form<UpdateData>,
) -> impl IntoResponse {
    sanitize_text(&mut data.nombre);
    sanitize_text(&mut data.mensaje);

    match sqlx::query("UPDATE mensajes SET nombre=$1, mensaje=$2 WHERE id=$3")
        .bind(&data.nombre)
        .bind(&data.mensaje)
        .bind(id)
        .execute(&pool)
        .await
    {
        Ok(_) => Html("✅ Mensaje actualizado"),
        Err(_) => Html("❌ Error al actualizar"),
    }
}

async fn delete_mensaje(State(pool): State<PgPool>, Path(id): Path<i32>) -> impl IntoResponse {
    match sqlx::query("DELETE FROM mensajes WHERE id = $1").bind(id).execute(&pool).await {
        Ok(_) => Html("✅ Mensaje eliminado"),
        Err(_) => Html("❌ Error al eliminar"),
    }
}

/* ---------- IMÁGENES & GALERÍA ---------- */

async fn upload_image(State(pool): State<PgPool>, mut multipart: Multipart) -> impl IntoResponse {
    tokio::fs::create_dir_all("uploads").await.unwrap();
    let mut file_saved = false;

    while let Some(field) = multipart.next_field().await.unwrap() {
        if field.name() != Some("file") { continue; }

        let mime = field.content_type().map(|m| m.to_string()).unwrap_or_default();
        if !ALLOWED_MIME.contains(&mime.as_str()) { return Html("❌ Tipo no permitido").into_response(); }

        let bytes = field.bytes().await.unwrap();
        if bytes.len() > MAX_IMAGE_SIZE { return Html("❌ Máximo 5MB").into_response(); }

        let extension = match mime.as_str() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/png" => "png",
            "image/webp" => "webp",
            _ => "jpg",
        };

        let filename = format!("{}.{}", Uuid::new_v4(), extension);
        let path = format!("uploads/{}", filename);

        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(&bytes).await.is_ok() {
                if sqlx::query("INSERT INTO images (filename) VALUES ($1)").bind(&filename).execute(&pool).await.is_ok() {
                    file_saved = true;
                }
            }
        }
    }
    if file_saved { Html("✅ Imagen subida").into_response() } 
    else { Html("❌ Error al guardar").into_response() }
}

async fn list_images(State(pool): State<PgPool>) -> Json<Vec<Image>> {
    let rows = sqlx::query("SELECT id, filename FROM images ORDER BY id DESC").fetch_all(&pool).await.unwrap();
    let images = rows.into_iter().map(|r| Image {
        id: r.get("id"),
        filename: r.get("filename"),
    }).collect();
    Json(images)
}

async fn eliminar_imagen_fisica(
    State(pool): State<PgPool>,
    Path(filename): Path<String>,
) -> impl IntoResponse {
    // 1. Borrar de la base de datos
    let db_res = sqlx::query("DELETE FROM images WHERE filename = $1").bind(&filename).execute(&pool).await;
    
    // 2. Borrar archivo físico
    let path = format!("uploads/{}", filename);
    let _ = tokio::fs::remove_file(path).await;

    match db_res {
        Ok(_) => Html("✅ Imagen eliminada"),
        Err(_) => Html("❌ Error en BD"),
    }
}

/* ---------- UTIL ---------- */

fn sanitize_text(text: &mut String) {
    let forbidden = ["<", ">", "\"", "'", ";", "--", "script"];
    for f in forbidden { *text = text.replace(f, ""); }
}