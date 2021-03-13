use crate::verification::SlackRequest;
use rocket::{get, post};
use rocket_contrib::json::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Test {
    test: String,
}

#[post("/test")]
pub fn test() -> String {
    println!("nothing");

    String::from("nothing")
}
