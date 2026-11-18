// a MongoDB database for the tests of code that needs database::DB. these tests
// are marked #[ignore], so a plain `cargo test` needs no MongoDB. run them with
// `TEST_DATABASE_URL=<uri> cargo test -- --include-ignored`, as CI does
use crate::database;

// the MongoDB the database tests create and drop their databases in. there's
// no default, so the tests never drop databases on a server nobody chose
pub fn url() -> String {
    std::env::var("TEST_DATABASE_URL").expect(
        "the database tests need TEST_DATABASE_URL, a MongoDB URI they may create and drop databases in",
    )
}

// an empty database of its own for the named test, so tests can run at the same
// time. the name is fixed per test and the database is dropped first, so one
// left behind by a failed test is cleared by the next run
pub async fn fresh(test: &str) -> database::DB {
    let url = url();
    let name = format!("ridgecast_test_{test}");
    let client = mongodb::Client::with_uri_str(&url)
        .await
        .expect("TEST_DATABASE_URL should be a MongoDB URI");
    client
        .database(&name)
        .drop()
        .await
        .expect("the database tests need a MongoDB at TEST_DATABASE_URL");
    database::DB::connect(&url, &name)
        .await
        .expect("the test database should connect")
}

// drops the test's database once the test has passed
pub async fn remove(db: database::DB) {
    let collection = &db.points_collection;
    collection
        .client()
        .database(&collection.namespace().db)
        .drop()
        .await
        .expect("the test database should drop");
}
