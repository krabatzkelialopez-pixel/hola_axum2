use axum::{
    extract::{Form, State, Multipart, Query, Path},
    routing::{get, post},
    response::{Html, IntoResponse},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::{env, net::SocketAddr, collections::HashMap};
use tower_http::{cors::CorsLayer, services::ServeDir};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use regex::Regex;

const MAX_IMAGE_SIZE: usize = 5 * 1024 * 1024;
const ALLOWED_MIME: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/jpg"];

/* ============================= */
/* ======= STRUCTS ============== */
/* ============================= */

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

/* ============================= */
/* ========= MAIN =============== */
/* ============================= */

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let pool = PgPool::connect(&env::var("DATABASE_URL").unwrap())
        .await
        .unwrap();

    let app = Router::new()

    /* ----------- FRONT ----------- */
    .route("/enviar", post(enviar))
    .route("/upload-image", post(upload_image))
    .route("/images", get(list_images))

    /* ----------- CRUD API ----------- */
    .route("/mensajes", get(list_mensajes))
    .route("/mensajes/:id", axum::routing::delete(delete_mensaje))
    .route("/mensajes/:id", axum::routing::put(update_mensaje))

    /* ----------- PANEL ADMIN ----------- */
    .route("/admin/dashboard", get(admin_dashboard))
    .route("/admin/mensajes", get(admin_mensajes_page))
    .route("/admin/images", get(admin_images_page))

    /* ----------- STATIC ----------- */
    .nest_service("/uploads", ServeDir::new("uploads"))
    .nest_service("/static", ServeDir::new("static"))
    .fallback_service(ServeDir::new("static"))

    .with_state(pool)
    .layer(CorsLayer::permissive());

    let port: u16 = env::var("PORT")
        .unwrap_or("8080".into())
        .parse()
        .unwrap();

    let addr = SocketAddr::from(([0,0,0,0], port));

    println!("Servidor corriendo en {}", addr);

    axum::serve(
        tokio::net::TcpListener::bind(addr).await.unwrap(),
        app
    )
    .await
    .unwrap();
}

/* ============================= */
/* ========= ADMIN HTML ========= */
/* ============================= */

async fn admin_dashboard() -> Html<&'static str> {
    Html(include_str!("../static/admin/dashboard.html"))
}

async fn admin_images_page() -> Html<&'static str> {
    Html(include_str!("../static/admin/images.html"))
}

/* ============================= */
/* ======= PAGINADO + SEARCH ==== */
/* ============================= */

async fn admin_mensajes_page(
    State(pool): State<PgPool>,
    Query(params): Query<HashMap<String,String>>,
) -> impl IntoResponse {

    let page: i64 = params
        .get("page")
        .unwrap_or(&"1".to_string())
        .parse()
        .unwrap_or(1);

    let buscar = params
        .get("buscar")
        .unwrap_or(&"".to_string())
        .to_string();

    let limit = 10;
    let offset = (page - 1) * limit;

    let rows = sqlx::query(
        "
        SELECT id,nombre,mensaje
        FROM mensajes
        WHERE nombre ILIKE $1
        ORDER BY id DESC
        LIMIT $2 OFFSET $3
        "
    )
    .bind(format!("%{}%", buscar))
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await
    .unwrap();

    let mut html = String::new();

    html.push_str("<h1>CRUD MENSAJES</h1>");

    html.push_str(r#"
        <form method="GET">
        <input name="buscar" placeholder="Buscar">
        <button>Buscar</button>
        </form>
    "#);

    html.push_str("<table border='1'>");

    for r in rows {
        let id:i32 = r.get("id");
        let nombre:String = r.get("nombre");
        let mensaje:String = r.get("mensaje");

        html.push_str(&format!(
            "<tr>
            <td>{}</td>
            <td>{}</td>
            <td>{}</td>
            </tr>",
            id,nombre,mensaje
        ));
    }

    html.push_str("</table>");

    html.push_str(&format!(
        r#"
        <br>
        <a href="?page={}">Anterior</a> |
        <a href="?page={}">Siguiente</a>
        "#,
        if page > 1 { page-1 } else { 1 },
        page+1
    ));

    Html(html)
}

/* ============================= */
/* ========= MENSAJES =========== */
/* ============================= */

async fn enviar(
    State(pool): State<PgPool>,
    Form(mut data): Form<FormData>,
) -> impl IntoResponse {

    sanitize_text(&mut data.nombre);
    sanitize_text(&mut data.mensaje);

    let name_re = Regex::new(r"^[a-zA-ZáéíóúÁÉÍÓÚñÑ\s]{3,50}$").unwrap();

    if !name_re.is_match(&data.nombre) {
        return Html("Nombre inválido");
    }

    if data.mensaje.len() < 10 {
        return Html("Mensaje muy corto");
    }

    if data.recaptcha.is_empty() {
        return Html("Falta captcha");
    }

    sqlx::query("INSERT INTO mensajes (nombre,mensaje) VALUES ($1,$2)")
        .bind(&data.nombre)
        .bind(&data.mensaje)
        .execute(&pool)
        .await
        .unwrap();

    Html("Mensaje guardado")
}

/* ============================= */
/* ========= UPDATE ============= */
/* ============================= */

async fn update_mensaje(
    State(pool): State<PgPool>,
    Path(id): Path<i32>,
    Form(mut data): Form<UpdateData>,
) -> impl IntoResponse {

    sanitize_text(&mut data.nombre);
    sanitize_text(&mut data.mensaje);

    sqlx::query(
        "UPDATE mensajes SET nombre=$1,mensaje=$2 WHERE id=$3"
    )
    .bind(&data.nombre)
    .bind(&data.mensaje)
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    Html("Actualizado")
}

/* ============================= */
/* ========= DELETE ============= */
/* ============================= */

async fn delete_mensaje(
    State(pool): State<PgPool>,
    Path(id): Path<i32>,
) -> impl IntoResponse {

    sqlx::query("DELETE FROM mensajes WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    Html("Eliminado")
}

/* ============================= */
/* ========= IMAGES ============= */
/* ============================= */

async fn upload_image(
    State(pool): State<PgPool>,
    mut multipart: Multipart,
) -> impl IntoResponse {

    tokio::fs::create_dir_all("uploads").await.unwrap();

    while let Some(field) = multipart.next_field().await.unwrap() {

        if field.name() != Some("file") {
            continue;
        }

        let mime = field.content_type().unwrap().to_string();

        if !ALLOWED_MIME.contains(&mime.as_str()) {
            return Html("Formato no permitido");
        }

        let bytes = field.bytes().await.unwrap();

        if bytes.len() > MAX_IMAGE_SIZE {
            return Html("Archivo muy grande");
        }

        let filename = format!("{}.jpg", Uuid::new_v4());
        let path = format!("uploads/{}", filename);

        let mut file = tokio::fs::File::create(&path).await.unwrap();
        file.write_all(&bytes).await.unwrap();

        sqlx::query("INSERT INTO images (filename) VALUES ($1)")
            .bind(&filename)
            .execute(&pool)
            .await
            .unwrap();
    }

    Html("Imagen subida")
}

async fn list_images(
    State(pool): State<PgPool>
) -> Json<Vec<Image>> {

    let rows = sqlx::query("SELECT id,filename FROM images ORDER BY id DESC")
        .fetch_all(&pool)
        .await
        .unwrap();

    let data = rows
        .into_iter()
        .map(|r| Image{
            id:r.get("id"),
            filename:r.get("filename"),
        })
        .collect();

    Json(data)
}

/* ============================= */
/* ========= LIST API =========== */
/* ============================= */

async fn list_mensajes(
    State(pool): State<PgPool>
) -> Json<Vec<Mensaje>> {

    let rows = sqlx::query(
        "SELECT id,nombre,mensaje FROM mensajes ORDER BY id DESC"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let data = rows
        .into_iter()
        .map(|r| Mensaje{
            id:r.get("id"),
            nombre:r.get("nombre"),
            mensaje:r.get("mensaje"),
        })
        .collect();

    Json(data)
}

/* ============================= */
/* ========= UTIL =============== */
/* ============================= */

fn sanitize_text(text: &mut String) {
    let forbidden = ["<",">","script",";","--"];
    for f in forbidden {
        *text = text.replace(f,"");
    }
}