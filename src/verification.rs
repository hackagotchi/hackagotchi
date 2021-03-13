use serde::de::DeserializeOwned;

use rocket::request::FormItems;
use rocket::Request;
use rocket::{
    data::{Data, FromData, Outcome, ToByteUnit},
    http::Status,
};

lazy_static::lazy_static! {
    pub static ref SIGNING_SECRET: String = std::env::var("SIGNING_SECRET").unwrap();
}

pub struct SlackRequest<T>(pub T);

impl<T> SlackRequest<T> {
    pub fn verify_request(body: &String) -> bool {
        false
    }
}

#[rocket::async_trait]
impl<'r, T: DeserializeOwned + Default> FromData for SlackRequest<T> {
    type Error = String;

    async fn from_data(req: &Request<'_>, data: Data) -> Outcome<Self, String> {
        let request_string = data.open(2.megabytes()).stream_to_string().await.unwrap();
        let parsed_request: T = serde_json::from_str(&request_string).unwrap();

        match req.headers().get_one("Content-Type").unwrap() {
            "application/json" => {
                Self::verify_request(&request_string);
            }
            "application/x-www-form-urlencoded" => (),
            _ => return Outcome::Failure((Status::BadRequest, String::from("error :("))),
        }

        Outcome::Success(Self(parsed_request))
    }
}
