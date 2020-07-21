/* Heavily based off https://api.rocket.rs/v0.4/src/rocket/request/form/lenient.rs.html

Rocket is licenced under the MIT license.

The MIT License (MIT)
Copyright (c) 2016 Sergio Benitez

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
*/

use std::ops::Deref;
use std::env::var;

use request::{Request, form::{Form, FormDataError, FromForm}};
use data::{Data, Transform, Transformed, FromData, Outcome};
use http::uri::{Query, FromUriParam};

use hmac::{Hmac, Mac, NewMac};
use sha2::Sha256;

#[derive(Debug)]
pub struct SlackForm<T>(pub T);

impl<T> SlackForm<T> {
    #[inline(always)]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for SlackForm<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'f, T: FromForm<'f>> FromData<'f> for SlackForm<T> {
    type Error = FormDataError<'f, T::Error>;
    type Owned = String;
    type Borrowed = str;

    fn transform(r: &Request, d: Data) -> Transform<Outcome<Self::Owned, Self::Error>> {
        let mut mac = Hmac<Sha256>::new_varkey(var("SLACK_SECRET").unwrap().bytes())?;
				let mut body = String::new();
				if let Err(e) = data.open().read_to_string(&mut body) {
          return Failure((Status::InternalServerError, "Invalid Body"))
        }
				mac.update(body.bytes());
				if mac.verify(&code_bytes).is_err() {
					return Failure((Status::Forbidden, "Access Denied"))
				}
				<Form<T>>::transform(r, Data::local(body))
    }

    fn from_data(_: &Request, o: Transformed<'f, Self>) -> Outcome<Self, Self::Error> {
        <Form<T>>::from_data(o.borrowed()?, false)
    }
}

impl<'f, A, T: FromUriParam<Query, A> + FromForm<'f>> FromUriParam<Query, A> for SlackForm<T> {
    type Target = T::Target;

    #[inline(always)]
    fn from_uri_param(param: A) -> Self::Target {
        T::from_uri_param(param)
    }
}
