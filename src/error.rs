use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

pub enum URLError {
    MalformedURL,
    Database(mongodb::error::Error),
    IncorrectPath,
    CollidedURL
}

impl IntoResponse for URLError {
    fn into_response(self) -> Response {
        match self {
            Self::MalformedURL => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "message": "URL provided was malformed" })),
            )
                .into_response(),
            Self::Database(err) => {
                tracing::error!(err = ?err);
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            },
            Self::IncorrectPath => StatusCode::NOT_FOUND.into_response(),
            Self::CollidedURL => StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

impl From<mongodb::error::Error> for URLError {
    fn from(err: mongodb::error::Error) -> Self {
        Self::Database(err)
    }
}
