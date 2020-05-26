/// This is like 99% copy pasted from https://api.rocket.rs/v0.4/src/rocket/request/form/lenient.rs.html.
use std::ops::Deref;

use rocket::request::{Request, form::{Form, FormDataError, FromForm}};
use rocket::data::{Data, Transform, Transformed, FromData, Outcome};
use rocket::http::uri::Uri;

#[derive(Debug)]
pub struct SlackSafety<T: FromData>(pub T);

impl<T> SlackSafety<T> {
    #[inline(always)]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for SlackSafety<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'f, T: FromData<'f>> FromData<'f> for SlackSafety<T> {
    type Error = FormDataError<'f, T::Error>;
    type Owned = String;
    type Borrowed = str;

    fn transform(r: &Request, d: Data) -> Transform<Outcome<Self::Owned, Self::Error>> {
        <T>::transform(r, d)
    }

    fn from_data(r: &Request, o: Transformed<'f, Self>) -> Outcome<Self, &str> {
        let raw_content = o.borrowed()?;
        let content = match Uri::percent_decode(raw_content).as_bytes() {
            Ok(decoded)  => decoded,
            Err(_error) => raw_content,
        };
        let res = reqwest::Client::new().post("https://slack.hosted.hackclub.com")
            .body(content)
            .send()
            .await?;
        if (!res.status().isSuccess()) {
            Outcome::Failure((Status::Ok, res.text().await?))
        } else {
            <T>::from_data(r, o)
        }
    }
}
