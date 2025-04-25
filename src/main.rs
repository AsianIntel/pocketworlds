#![deny(clippy::all, clippy::pedantic)]

mod database;
mod error;
mod models;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::Path,
    http::{StatusCode, header::LOCATION},
    response::Response,
    routing::{get, post},
};
use bson::oid::ObjectId;
use database::Database;
use error::URLError;
use models::{LeaderboardItem, ShortenedURL};
use redis::{
    AsyncCommands, Client as RedisClient, FromRedisValue, PushKind,
    aio::MultiplexedConnection as RedisConnection,
};
use serde::{Deserialize, Serialize};
use std::{env, time::Duration};
use tokio::{net::TcpListener, time::interval};
use tower_http::trace::TraceLayer;
use url::Url;
use uuid::Uuid;

#[derive(Deserialize)]
struct URLParams {
    pub url: String,
}

#[derive(Serialize)]
struct URLResponse {
    pub url: String,
}

async fn shorten_url_route(
    database: Extension<Database>,
    mut redis: Extension<RedisConnection>,
    params: Json<URLParams>,
) -> Result<Json<URLResponse>, URLError> {
    let Ok(url) = params.url.parse::<Url>() else {
        return Err(URLError::MalformedURL);
    };
    if !matches!(url.scheme(), "https" | "http") {
        return Err(URLError::MalformedURL);
    }

    let id = url_to_uuid(&url);
    let encoded_url = uuid_to_path(id);
    let existing = database
        .get_shortened_url(bson::Uuid::from_bytes(id.into_bytes()), &encoded_url)
        .await?;
    if let Some(existing) = existing {
        if existing.long_url == url {
            return Ok(Json(URLResponse {
                url: existing.shortened_url,
            }));
        }
        return Err(URLError::CollidedURL);
    }

    let url = ShortenedURL {
        _id: ObjectId::new(),
        idx: bson::Uuid::from_bytes(id.into_bytes()),
        long_url: url,
        shortened_url: encoded_url.clone(),
    };
    // If you are creating a shortened url, you are most likely using it immediately. So, just pre-add it to the cache.
    if let Err(err) = redis
        .set::<'_, _, _, ()>(encoded_url.clone(), serde_json::to_vec(&url).unwrap())
        .await
    {
        tracing::error!(err = ?err);
    }
    if let Err(err) = redis
        .publish::<'_, _, _, ()>("url_insert", serde_json::to_vec(&url).unwrap())
        .await
    {
        tracing::error!(err = ?err);
    }

    Ok(Json(URLResponse { url: encoded_url }))
}

async fn get_shortened_url(
    database: Extension<Database>,
    mut redis: Extension<RedisConnection>,
    path: Path<String>,
) -> Result<Response, URLError> {
    let uuid = path_to_uuid(&path.0).ok_or(URLError::IncorrectPath)?;

    match redis.get::<'_, _, Option<Vec<u8>>>(&path.0).await {
        Ok(Some(data)) => {
            let url = serde_json::from_slice::<ShortenedURL>(&data).unwrap();
            if let Err(err) = redis
                .publish::<'_, _, _, ()>("visits", &url._id.bytes())
                .await
            {
                tracing::error!(err = ?err);
            }

            return Ok(Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(LOCATION, url.long_url.to_string())
                .body(Body::empty())
                .unwrap());
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(err = ?err);
        }
    }

    let existing = database
        .get_shortened_url(bson::Uuid::from_bytes(uuid.into_bytes()), &path.0)
        .await?;
    if let Some(existing) = existing {
        if let Err(err) = redis
            .set::<'_, _, _, ()>(path.0, serde_json::to_vec(&existing).unwrap())
            .await
        {
            tracing::error!(err = ?err);
        }
        if let Err(err) = redis
            .publish::<'_, _, _, ()>("visits", &existing._id.bytes())
            .await
        {
            tracing::error!(err = ?err);
        }

        return Ok(Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(LOCATION, existing.long_url.to_string())
            .body(Body::empty())
            .unwrap());
    }
    Err(URLError::IncorrectPath)
}

async fn get_leaderboard_route(
    database: Extension<Database>,
    mut redis: Extension<RedisConnection>,
) -> Result<Json<Vec<LeaderboardItem>>, URLError> {
    match redis.get::<'_, _, Option<Vec<u8>>>("leaderboard").await {
        Ok(Some(leaderboard)) => {
            if let Ok(leaderboard) =
                serde_json::from_slice::<'_, Vec<LeaderboardItem>>(&leaderboard)
            {
                return Ok(Json(leaderboard));
            }
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(err = ?err);
        }
    }

    let urls = database.get_url_leaderboard().await?;
    if let Err(err) = redis
        .set::<'_, _, _, ()>("leaderboard", serde_json::to_vec(&urls).unwrap())
        .await
    {
        tracing::error!(err = ?err);
    }

    Ok(Json(urls))
}

#[tracing::instrument(skip_all)]
async fn refresh_leaderboard(database: Database, mut redis: RedisConnection) {
    let mut interval = interval(Duration::from_secs(60));
    loop {
        tracing::debug!("refreshing leaderboard");
        match database.get_url_leaderboard().await {
            Ok(urls) => {
                if let Err(err) = redis
                    .set::<'_, _, _, ()>("leaderboard", serde_json::to_vec(&urls).unwrap())
                    .await
                {
                    tracing::error!(err = ?err);
                }
            }
            Err(err) => {
                tracing::error!(err = ?err);
            }
        }

        interval.tick().await;
    }
}

#[tracing::instrument(skip_all)]
async fn url_insert_queue(redis: RedisClient, database: Database) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let config = redis::AsyncConnectionConfig::new().set_push_sender(tx);
    let mut conn = redis
        .get_multiplexed_async_connection_with_config(&config)
        .await
        .unwrap();
    conn.subscribe("url_insert").await.unwrap();

    loop {
        if let Some(data) = rx.recv().await {
            if data.kind == PushKind::Message {
                if let Ok(data) = Vec::<u8>::from_redis_value(&data.data[1]) {
                    let payload = serde_json::from_slice::<ShortenedURL>(&data).unwrap();
                    if let Err(err) = database.insert_shortened_url(&payload).await {
                        tracing::error!(err = ?err);
                    }
                }
            }
        }
    }
}

#[tracing::instrument(skip_all)]
async fn visits_queue(redis: RedisClient, database: Database) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let config = redis::AsyncConnectionConfig::new().set_push_sender(tx);
    let mut conn = redis
        .get_multiplexed_async_connection_with_config(&config)
        .await
        .unwrap();
    conn.subscribe("visits").await.unwrap();

    loop {
        if let Some(data) = rx.recv().await {
            if data.kind == PushKind::Message {
                if let Ok(data) = <[u8; 12]>::from_redis_value(&data.data[1]) {
                    if let Err(err) = database.insert_visit(ObjectId::from_bytes(data)).await {
                        tracing::error!(err = ?err);
                    }
                }
            }
        }
    }
}

#[must_use]
pub fn path_to_uuid(path: &str) -> Option<Uuid> {
    let id = base62::decode(path).ok()?;
    Some(Uuid::from_u128(id))
}

#[must_use]
pub fn uuid_to_path(uuid: Uuid) -> String {
    base62::encode(uuid.as_u128())
}

#[must_use]
pub fn url_to_uuid(url: &Url) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, url.as_str().as_bytes())
}

/// Sets up the database, cache and recurring tasks.
/// 
/// # Panics
/// 
/// If any of the environment variables required are not found, the code will panic with an error message.
pub async fn setup() -> Router {
    let mongodb_url =
        env::var("MONGODB_URL").expect("Expected MONGODB_URL as an environment variable");
    let redis_url = env::var("REDIS_URL").expect("Expected REDIS_URL as an environment variable");

    let database = Database::new(&mongodb_url).await.unwrap();
    let redis = RedisClient::open(redis_url).unwrap();
    let connection = redis.get_multiplexed_async_connection().await.unwrap();

    let app = Router::new()
        .route("/shorten", post(shorten_url_route))
        .route("/url-leaderboard", get(get_leaderboard_route))
        .route("/{path}", get(get_shortened_url))
        .layer(Extension(database.clone()))
        .layer(Extension(connection.clone()))
        .layer(TraceLayer::new_for_http());

    tokio::spawn(refresh_leaderboard(database.clone(), connection));
    tokio::spawn(url_insert_queue(redis.clone(), database.clone()));
    tokio::spawn(visits_queue(redis, database));

    app
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let app = setup().await;

    let port = env::var("PORT").expect("Expected PORT as an environment variable");
    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();

    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::Request};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn shorten_url_valid() {
        let app = test_app().await;

        let body = json!({ "url": "https://example.com" });
        let response = app
            .oneshot(
                Request::post("/shorten")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body_bytes).unwrap();
        assert!(json["url"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn shorten_url_invalid() {
        let app = test_app().await;

        let body = json!({ "url": "/invalid" });
        let response = app
            .oneshot(
                Request::post("/shorten")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn uuid_encode_decode_roundtrip() {
        let original = url_to_uuid(&Url::parse("https://example.com").unwrap());
        let encoded = uuid_to_path(original);
        let decoded = path_to_uuid(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[tokio::test]
    async fn url_to_uuid_determinism() {
        let url1 = Url::parse("https://example.com").unwrap();
        let url2 = Url::parse("https://example.com").unwrap();
        assert_eq!(url_to_uuid(&url1), url_to_uuid(&url2));
    }

    #[tokio::test]
    async fn redirect_existing_path() {
        let app = test_app().await;

        let body = json!({ "url": "https://example.com/test-redirect" });
        let response = app
            .clone()
            .oneshot(
                Request::post("/shorten")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice::<Value>(&body_bytes).unwrap();
        let path = json["url"].as_str().unwrap();

        let redirect_response = app
            .oneshot(
                Request::get(format!("/{}", path))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(redirect_response.status(), StatusCode::TEMPORARY_REDIRECT);
        let headers = redirect_response.headers();
        assert_eq!(
            headers.get("location").unwrap(),
            "https://example.com/test-redirect"
        );
    }

    #[tokio::test]
    async fn redirect_invalid_path() {
        let app = test_app().await;
        let response = app
            .oneshot(Request::get("/invalidpath").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    async fn test_app() -> Router {
        dotenvy::dotenv().unwrap();

        setup().await
    }
}
