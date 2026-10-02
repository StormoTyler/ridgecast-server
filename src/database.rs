use chrono::prelude::*;
use futures::StreamExt;
use mongodb::bson::{doc, oid::ObjectId, Document};
use mongodb::options::{FindOneAndUpdateOptions, FindOptions, IndexOptions, ReturnDocument};
use mongodb::{bson, options::ClientOptions, Client, Collection, IndexModel};
use std::str::FromStr;

// local modules
mod api;

#[derive(Clone, Debug)]
pub struct DB {
    pub alerts_collection: Collection<api::nws::AlertsSorted>,
    pub avalanche_collection: Collection<api::nac::AvalancheRisk>,
    pub points_collection: Collection<UserPoint>,
    pub users_collection: Collection<User>,
}

#[derive(Debug)]           
pub struct UserPoint       
{                          
    user: String,          
    point: api::nws::Point,
}                          
                           
#[derive(Debug)]           
pub struct User            
{                          
    user_id: String,       
    email: String,         
    phone: String,         
}                          

type Result<T> = std::result::Result<T, MyError>;

impl DB {
    pub async fn init() -> Result<Self> {
        let mongodb_uri = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set.");
        let database_name =
            std::env::var("MONGO_INITDB_DATABASE")
            .expect("MONGO_INITDB_DATABASE must be set.");
        let collection_name =
            std::env::var("MONGODB_NOTE_COLLECTION")
            .expect("MONGODB_NOTE_COLLECTION must be set.");

        let mut client_options = ClientOptions::parse(mongodb_uri).await?;
        client_options.app_name = Some(database_name.to_string());

        let client = Client::with_options(client_options)?;
        let database = client.database(database_name.as_str());

        let note_collection = database.collection(collection_name.as_str());
        let collection = database.collection::<Document>(collection_name.as_str());

        println!("Database connected successfully");

        Ok(Self {
            note_collection,
            collection,
        })
    }
   }

