use mongodb::{bson::doc, error::Result, options::ClientOptions, Client, Collection};

// local modules
use crate::models::{AlertsSorted, UserPoint, AvalancheData};

#[derive(Clone, Debug)]
pub struct DB {
    pub alerts_collection: Collection<AlertsSorted>,
    pub points_collection: Collection<UserPoint>,
    pub avy_collection: Collection<AvalancheData>,
}

impl DB {
    pub async fn init() -> Result<Self> {
        let mongodb_uri = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017".to_string());
        let database_name = std::env::var("TRAIL_ALERT_DB")
            .unwrap_or_else(|_| "trail-alert".to_string());

        let mut client_options = ClientOptions::parse(mongodb_uri).await?;
        client_options.app_name = Some(database_name.clone());

        let client = Client::with_options(client_options)?;
        let database = client.database(database_name.as_str());

        // pinging the database
        database.run_command(doc! { "ping": 1 }).await?;

        let alerts_collection = database.collection::<AlertsSorted>("alerts");
        let points_collection = database.collection::<UserPoint>("points");
        let avy_collection = database.collection::<AvalancheData>("avy");

        println!("Pinged your database. Successfully connected to MongoDB!");

        Ok(Self {
            alerts_collection,
            points_collection,
            avy_collection,
        })
    }
}
