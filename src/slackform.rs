use std::env::var;

use request::{Request, form::{Form, FormDataError, FromForm}};
use data::{Data, Transform, Transformed, FromData, Outcome};

use hmac::{Hmac, Mac, NewMac};
use sha2::Sha256;

pub struct SlackForm<T> where T: FromData (pub T);

impl<'f, T> FromData<'f> for SlackForm<T> where 
	T: FromData<'f> {
    fn transform(r: &Request, d: Data) -> Transform<Outcome<Self::Owned, Self::Error>> {
				let mut body = String::new();
				if data.open().read_to_string(&mut body).is_err() {
          return Failure((Status::BadRequest, "Invalid Body"))
        }
				let mut mac = Hmac<Sha256>::new_varkey(var("SLACK_SECRET").unwrap().bytes())?;
				mac.update(body.bytes());
				if mac.verify(&code_bytes).is_err() {
					return Failure((Status::Forbidden, "Access Denied"))
				}
				T::transform(r, Data::local(body))
    }

    fn from_data(r: &Request, t: Transformed<'f, Self>) -> Outcome<Self, Self::Error> {
        T::from_data(r, t)
    }
}
