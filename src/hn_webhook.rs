use log::info;
use rocket::post;

#[post("/hn/transaction")]
pub async fn transaction() {
    info!("transaction!");
}

#[post("/hn/payment")]
pub async fn payment() {
    info!("payment!");
}
