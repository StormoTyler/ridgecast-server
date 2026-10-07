use mongodb::{bson::doc, error::Result, options::ClientOptions, Client, Collection, IndexModel};

// local modules
use crate::models::{AlertsSorted, UserPoint, AvyHashMap};

#[derive(Clone, Debug)]
pub struct DB {
    pub alerts_collection: Collection<AlertsSorted>,
    pub points_collection: Collection<UserPoint>,
    pub avy_collection: Collection<AvyHashMap>,
}

impl DB {
    pub async fn init() -> Result<Self> {
        let mongodb_uri = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017".to_string());
        let database_name = std::env::var("RIDGECAST_DB")
            .unwrap_or_else(|_| "ridgecast".to_string());

        let mut client_options = ClientOptions::parse(mongodb_uri).await?;
        client_options.app_name = Some(database_name.clone());

        let client = Client::with_options(client_options)?;
        let database = client.database(database_name.as_str());

        // pinging the database
        database.run_command(doc! { "ping": 1 }).await?;

        let alerts_collection = database.collection::<AlertsSorted>("alerts");
        let points_collection = database.collection::<UserPoint>("points");
        let avy_collection = database.collection::<AvyHashMap>("avy");

        // the scheduler finds due daily forecasts with next_daily_forecast <= now.
        // creating an index that already exists does nothing
        points_collection
            .create_index(IndexModel::builder().keys(doc! { "next_daily_forecast": 1 }).build())
            .await?;

        println!("Pinged your database. Successfully connected to MongoDB!");

        Ok(Self {
            alerts_collection,
            points_collection,
            avy_collection,
        })
    }
}
