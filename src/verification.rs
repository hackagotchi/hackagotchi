use serde::de::DeserializeOwned;

use rocket::Request;
use rocket::{
    data::{Data, FromData, Outcome, ToByteUnit},
    http::Status,
};

use hmac::{Hmac, Mac, NewMac};
use sha2::Sha256;

use hex::FromHex;

// Create alias for HMAC-SHA256
type HmacSha256 = Hmac<Sha256>;

lazy_static::lazy_static! {
    pub static ref SIGNING_SECRET: String = std::env::var("SIGNING_SECRET").unwrap();
}

pub struct SlackRequest<T>(pub T);

impl<T> SlackRequest<T> {
    pub fn verify_request(body: &String, timestamp: &String, signature: &String) -> bool {
        let basestring = format!("v0:{}:{}", timestamp, body);
        let signature_bytes =
            Vec::<u8>::from_hex(&signature.strip_prefix("v0=").unwrap_or("")).unwrap();

        let mut mac = HmacSha256::new_varkey(SIGNING_SECRET.as_bytes()).unwrap();
        mac.update(basestring.as_bytes());

        mac.verify(&signature_bytes[..]).is_ok()
    }
}

#[rocket::async_trait]
impl<'r, T: DeserializeOwned> FromData for SlackRequest<T> {
    type Error = String;

    async fn from_data(req: &Request<'_>, data: Data) -> Outcome<Self, String> {
        let request_string = data.open(2.megabytes()).stream_to_string().await.unwrap();
        let parsed_request: T = serde_json::from_str(&request_string).unwrap();

        let timestamp = req
            .headers()
            .get_one("X-Slack-Request-Timestamp")
            .expect("No request timestamp");
        let signature = req
            .headers()
            .get_one("X-Slack-Signature")
            .expect("No signature in request");

        if !Self::verify_request(
            &request_string,
            &timestamp.to_string(),
            &signature.to_string(),
        ) {
            return Outcome::Failure((Status::BadRequest, String::from("request not verified")));
        }

        match req.headers().get_one("Content-Type").unwrap() {
            "application/json" => {}
            "application/x-www-form-urlencoded" => (),
            _ => return Outcome::Failure((Status::BadRequest, String::from("error :("))),
        }

        Outcome::Success(Self(parsed_request))
    }
}
