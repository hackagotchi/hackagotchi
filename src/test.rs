use crate::verification::SlackRequest;
use rocket::{get, post};
use rocket_contrib::json::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Test {
    team_id: String,
}

#[post("/test", data = "<request>")]
pub fn test(request: SlackRequest<Test>) -> String {
    println!("nothing");

    request.0.team_id
}
