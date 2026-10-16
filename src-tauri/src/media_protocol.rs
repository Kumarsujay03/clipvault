//! `cvmedia://localhost/{thumb|full}/<id>` — serves image bytes for an item id. The frontend
//! never sees filesystem paths; the backend resolves and validates them.

use std::sync::Arc;

use tauri::http::{Request, Response, StatusCode};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::app::AppState;
use clipvault_core::media::mime_for_ext;

pub fn handler<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let Some(state) = ctx
        .app_handle()
        .try_state::<Arc<AppState>>()
        .map(|s| s.inner().clone())
    else {
        responder.respond(status(StatusCode::SERVICE_UNAVAILABLE));
        return;
    };
    let path = req.uri().path().to_owned();
    std::thread::spawn(move || responder.respond(serve(&state, &path)));
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap()
}

fn serve(state: &AppState, path: &str) -> Response<Vec<u8>> {
    let mut parts = path.trim_start_matches('/').split('/');
    let (Some(kind), Some(id), None) = (parts.next(), parts.next(), parts.next()) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Ok(id) = id.parse::<i64>() else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Ok((media, thumb, mime)) = state.store.media_paths(id) else {
        return status(StatusCode::NOT_FOUND);
    };
    let (rel, content_type) = match kind {
        "thumb" => match thumb {
            Some(t) => (t, "image/png".to_string()),
            None => return status(StatusCode::NOT_FOUND),
        },
        "full" => match media {
            Some(m) => {
                let ext = m.rsplit('.').next().unwrap_or("");
                let ct = if mime.starts_with("image/") {
                    mime
                } else {
                    mime_for_ext(ext).to_string()
                };
                (m, ct)
            }
            None => return status(StatusCode::NOT_FOUND),
        },
        _ => return status(StatusCode::BAD_REQUEST),
    };
    match state.media.read(&rel) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Cache-Control", "private, max-age=86400")
            .body(bytes)
            .unwrap(),
        Err(_) => status(StatusCode::NOT_FOUND),
    }
}
