use bson::{oid::ObjectId, Document};
use chrono::Utc;
use futures_util::TryStreamExt;
use mongodb::{
    Client,
    error::Error,
    options::{ClientOptions, ServerApi, ServerApiVersion},
};

use crate::models::{AggregationResult, LeaderboardItem, ShortenedURL};

#[derive(Clone)]
pub struct Database {
    client: Client,
}

impl Database {
    pub async fn new(connection_string: &str) -> Result<Self, Error> {
        let mut client_options = ClientOptions::parse(connection_string).await?;
        let server_api = ServerApi::builder().version(ServerApiVersion::V1).build();
        client_options.server_api = Some(server_api);

        let client = Client::with_options(client_options).unwrap();

        Ok(Self { client })
    }

    pub async fn get_shortened_url(&self, id: bson::Uuid, shortened_url: &str) -> Result<Option<ShortenedURL>, Error> {
        let urls = self
            .client
            .database("db")
            .collection::<ShortenedURL>("urls");
        let url = urls.find_one(bson::doc! { "idx": id, "shortened_url": shortened_url }).await?;

        Ok(url)
    }

    pub async fn insert_shortened_url(&self, url: &ShortenedURL) -> Result<(), Error> {
        let urls = self
            .client
            .database("db")
            .collection::<ShortenedURL>("urls");
        urls.insert_one(url).await?;

        Ok(())
    }

    pub async fn insert_visit(&self, id: ObjectId) -> Result<(), Error> {
        let visits = self.client.database("db").collection::<Document>("visits");

        visits
            .insert_one(bson::doc! { "timestamp": Utc::now(), "id": id })
            .await?;

        Ok(())
    }

    pub async fn get_url_leaderboard(&self) -> Result<Vec<LeaderboardItem>, Error> {
        let pipeline = vec![
            bson::doc! {
                "$group": {
                    "_id": "$id",
                    "count": { "$sum": 1 }
                }
            },
            bson::doc! {
                "$sort": { "count": -1 }
            },
            bson::doc! {
                "$limit": 10
            },
        ];
        let visits = self.client.database("db").collection::<Document>("visits");
        let urls = self
            .client
            .database("db")
            .collection::<ShortenedURL>("urls");

        let mut cursor = visits.aggregate(pipeline).await?;
        let mut routes = Vec::new();
        while let Some(result) = cursor.try_next().await? {
            let route = bson::from_document::<AggregationResult>(result).unwrap();
            routes.push(route);
        }

        let mut results = Vec::new();
        for route in routes {
            let url = urls.find_one(bson::doc! { "_id": route._id }).await?;
            if let Some(url) = url {
                results.push(LeaderboardItem {
                    url: url.long_url.to_string(),
                    count: route.count,
                });
            }
        }

        Ok(results)
    }
}
