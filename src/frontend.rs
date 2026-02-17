use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};

#[cfg(embed_frontend)]
use include_dir::{Dir, include_dir};
#[cfg(embed_frontend)]
use mime_guess::MimeGuess;

#[cfg(embed_frontend)]
static FRONTEND_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/frontend/dist");

#[cfg(embed_frontend)]
fn asset_response(path: &str) -> Response {
    match FRONTEND_DIR.get_file(path) {
        Some(file) => {
            let mime: MimeGuess = mime_guess::from_path(path);
            let content_type = mime.first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type.as_ref())
                .body(Body::from(file.contents()))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn frontend_fallback(req: Request<Body>) -> Response {
    if req.method() != Method::GET {
        return StatusCode::NOT_FOUND.into_response();
    }

    #[cfg(not(embed_frontend))]
    {
        let _ = req;
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/plain")
            .body(Body::from("Frontend not embedded. Use Vite dev server."))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    #[cfg(embed_frontend)]
    {
        let path = req.uri().path().trim_start_matches('/');
        if path.is_empty() {
            return asset_response("index.html");
        }
        if path == "api" || path.starts_with("api/") {
            return StatusCode::NOT_FOUND.into_response();
        }

        if FRONTEND_DIR.get_file(path).is_some() {
            return asset_response(path);
        }

        asset_response("index.html")
    }
}
