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
use serde::Serialize;
use axum::extract::Path;



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

/* ============================= */
/* ===== NUEVO PARA UPDATE ===== */
/* ============================= */

#[derive(Deserialize)]
struct UpdateData {
    nombre: String,
    mensaje: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let pool = PgPool::connect(&env::var("DATABASE_URL").unwrap()).await.unwrap();

    let app = Router::new()
    .route("/enviar", post(enviar))
    .route("/upload-image", post(upload_image))
    .route("/images", get(list_images))

    // ===== CRUD MENSAJES =====
    .route("/mensajes", get(list_mensajes))
    .route("/mensajes/:id", axum::routing::delete(delete_mensaje))
    .route("/mensajes/:id", axum::routing::put(update_mensaje)) // 👈 AGREGADO

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

/* ============================= */
/* ===== FUNCIÓN UPDATE ========= */
/* ============================= */

async fn update_mensaje(
    State(pool): State<PgPool>,
    Path(id): Path<i32>,
    Form(mut data): Form<UpdateData>,
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

    match sqlx::query("UPDATE mensajes SET nombre=$1, mensaje=$2 WHERE id=$3")
        .bind(&data.nombre)
        .bind(&data.mensaje)
        .bind(id)
        .execute(&pool)
        .await
    {
        Ok(_) => Html("✅ Mensaje actualizado correctamente"),
        Err(_) => Html("❌ Error al actualizar mensaje"),
    }
}

/* ---------- SUBIR IMÁGENES ---------- */

async fn upload_image(
    State(pool): State<PgPool>,
    mut multipart: Multipart,
) -> impl IntoResponse {

    tokio::fs::create_dir_all("uploads").await.unwrap();

    let mut file_saved = false; // Bandera para saber si realmente guardamos algo

    while let Some(field) = multipart.next_field().await.unwrap() {
        
        // CORRECCIÓN AQUÍ: Cambiamos "image" por "file" para coincidir con el HTML
        if field.name() != Some("file") {
            continue;
        }

        let mime = field
            .content_type()
            .map(|m| m.to_string())
            .unwrap_or_default();

        if !ALLOWED_MIME.contains(&mime.as_str()) {
            return Html("❌ Tipo de archivo no permitido").into_response();
        }

        // field.bytes() consume el field, así que lo hacemos aquí
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

        // Guardar archivo en disco
        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(&bytes).await.is_ok() {
                // Guardar referencia en BD
                let insert_result = sqlx::query("INSERT INTO images (filename) VALUES ($1)")
                    .bind(&filename)
                    .execute(&pool)
                    .await;

                if insert_result.is_ok() {
                    file_saved = true;
                }
            }
        }
    }

    if file_saved {
        Html("✅ Imagen subida correctamente").into_response()
    } else {
        Html("❌ No se pudo guardar la imagen (Error interno o campo incorrecto)").into_response()
    }
}

/* ---------- LISTAR ---------- */

async fn list_mensajes(State(pool): State<PgPool>) -> Json<Vec<Mensaje>> {
    let rows = sqlx::query("SELECT id, nombre, mensaje FROM mensajes ORDER BY id DESC")
        .fetch_all(&pool)
        .await
        .unwrap();

    let data = rows
        .into_iter()
        .map(|r| Mensaje {
            id: r.get("id"),
            nombre: r.get("nombre"),
            mensaje: r.get("mensaje"),
        })
        .collect();

    Json(data)
}

#[derive(Serialize)]
struct Image {
    id: i32,
    filename: String,
}

async fn list_images(State(pool): State<PgPool>) -> Json<Vec<Image>> {
    let rows = sqlx::query("SELECT id, filename FROM images ORDER BY id DESC")
        .fetch_all(&pool)
        .await
        .unwrap();

    let images = rows
        .into_iter()
        .map(|r| Image {
            id: r.get("id"),
            filename: r.get("filename"),
        })
        .collect();

    Json(images)
}

/* ---------- UTIL ---------- */

fn sanitize_text(text: &mut String) {
    let forbidden = ["<", ">", "\"", "'", ";", "--", "script"];
    for f in forbidden {
        *text = text.replace(f, "");
    }
}

async fn delete_mensaje(
    State(pool): State<PgPool>,
    Path(id): Path<i32>,
) -> impl IntoResponse {
    match sqlx::query("DELETE FROM mensajes WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
    {
        Ok(_) => Html("✅ Mensaje eliminado"),
        Err(_) => Html("❌ Error al eliminar"),
    }
}
