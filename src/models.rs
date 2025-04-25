use bson::oid::ObjectId;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Deserialize, Serialize)]
pub struct ShortenedURL {
    #[allow(clippy::used_underscore_binding)]
    pub _id: ObjectId,
    pub idx: bson::Uuid,
    pub long_url: Url,
    pub shortened_url: String,
}

#[derive(Deserialize)]
pub struct AggregationResult {
    #[allow(clippy::used_underscore_binding)]
    pub _id: ObjectId,
    pub count: u32,
}

#[derive(Deserialize, Serialize)]
pub struct LeaderboardItem {
    pub url: String,
    pub count: u32,
}
