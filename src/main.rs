use axum::{
    extract::{Form, State, Multipart, Path, Query}, // Agregado Query
    routing::{get, post, delete, put}, // Agregado delete y put explícitamente
    response::{Html, IntoResponse},
    Json, Router,
};
use serde::{Deserialize, Serialize}; // Agregado Serialize
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

// --- ESTRUCTURAS NUEVAS PARA PAGINACIÓN ---
#[derive(Deserialize)]
struct PaginationParams {
    page: Option<i64>,
    limit: Option<i64>,
}

#[derive(Serialize)]
struct PaginatedResponse {
    data: Vec<Mensaje>,
    total: i64,
    page: i64,
    total_pages: i64,
}
// ------------------------------------------

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let pool = PgPool::connect(&env::var("DATABASE_URL").unwrap()).await.unwrap();

    let app = Router::new()
        .route("/", get(sirve_inicio)) // Ruta raíz
        .route("/admin", get(serve_admin)) // <--- NUEVA RUTA ADMIN
        .route("/enviar", post(enviar))
        .route("/upload-image", post(upload_image))
        .route("/images", get(list_images))

        // ===== CRUD MENSAJES =====
        .route("/mensajes", get(list_mensajes))
        .route("/mensajes/:id", delete(delete_mensaje))
        .route("/mensajes/:id", put(update_mensaje))

        .nest_service("/uploads", ServeDir::new("uploads"))
        .nest_service("/static", ServeDir::new("static")) // Servir estáticos generales
        .nest_service("/css", ServeDir::new("static/css")) // Servir CSS explícitamente
        .nest_service("/img", ServeDir::new("static/img")) // Servir Imágenes explícitamente
        .with_state(pool)
        .layer(CorsLayer::permissive());

    let port: u16 = env::var("PORT").unwrap_or("3000".into()).parse().unwrap();
    let addr = SocketAddr::from(([0,0,0,0], port));
    
    println!("Servidor corriendo en puerto {}", port);

    axum::serve(tokio::net::TcpListener::bind(addr).await.unwrap(), app).await.unwrap();
}

// --- FUNCIÓN PARA SERVIR HTML ---
async fn sirve_inicio() -> Html<String> {
    match tokio::fs::read_to_string("static/index.html").await {
        Ok(html) => Html(html),
        Err(_) => Html("<h1>Error cargando index.html</h1>".to_string()),
    }
}

// --- NUEVA FUNCIÓN PARA SERVIR ADMIN ---
async fn serve_admin() -> Html<String> {
    match tokio::fs::read_to_string("static/admin.html").await {
        Ok(html) => Html(html),
        Err(_) => Html("<h1>Error cargando admin.html</h1>".to_string()),
    }
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

/* ---------- SUBIR IMÁGENES (CORREGIDO) ---------- */

async fn upload_image(
    State(pool): State<PgPool>,
    mut multipart: Multipart,
) -> impl IntoResponse {

    tokio::fs::create_dir_all("uploads").await.unwrap();
    let mut file_saved = false;

    while let Some(field) = multipart.next_field().await.unwrap() {
        if field.name() != Some("file") { continue; } // Debe coincidir con el name="file" del HTML

        let mime = field.content_type().map(|m| m.to_string()).unwrap_or_default();
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

        if let Ok(mut file) = tokio::fs::File::create(&path).await {
            if file.write_all(&bytes).await.is_ok() {
                let _ = sqlx::query("INSERT INTO images (filename) VALUES ($1)")
                    .bind(&filename)
                    .execute(&pool)
                    .await;
                file_saved = true;
            }
        }
    }

    if file_saved {
        Html("✅ Imagen subida correctamente").into_response()
    } else {
        Html("❌ Error al guardar imagen").into_response()
    }
}

/* ---------- LISTAR CON PAGINACIÓN (MODIFICADO) ---------- */

async fn list_mensajes(
    State(pool): State<PgPool>,
    Query(params): Query<PaginationParams>, // Recibimos params de URL
) -> Json<PaginatedResponse> {
    
    let page = params.page.unwrap_or(1).max(1); // Página mínima 1
    let limit = params.limit.unwrap_or(5).max(1); // Default 5 items
    let offset = (page - 1) * limit;

    // 1. Contar total de mensajes
    let count_result: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM mensajes")
        .fetch_one(&pool)
        .await
        .unwrap_or((0,));
    let total = count_result.0;

    // 2. Obtener los mensajes de esta página
    let rows = sqlx::query("SELECT id, nombre, mensaje FROM mensajes ORDER BY id DESC LIMIT $1 OFFSET $2")
        .bind(limit)
        .bind(offset)
        .fetch_all(&pool)
        .await
        .unwrap();

    let data: Vec<Mensaje> = rows
        .into_iter()
        .map(|r| Mensaje {
            id: r.get("id"),
            nombre: r.get("nombre"),
            mensaje: r.get("mensaje"),
        })
        .collect();

    let total_pages = (total as f64 / limit as f64).ceil() as i64;

    Json(PaginatedResponse {
        data,
        total,
        page,
        total_pages,
    })
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