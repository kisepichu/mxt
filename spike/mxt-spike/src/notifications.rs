//! `notifications [--limit N]`: question (4), whether `GET /notifications` works.

use anyhow::Result;
use matrix_sdk::ruma::{UInt, api::client::push::get_notifications};

use crate::{
    session::{self, DataDir},
    util,
};

pub async fn run(dir: &DataDir, limit: u32) -> Result<()> {
    let session = session::restore(dir).await?;
    let mut request = get_notifications::v3::Request::new();
    request.limit = Some(UInt::from(limit));
    let response = match session.client.send(request).await {
        Ok(response) => response,
        Err(error) => {
            println!("GET /notifications failed: {error}");
            if let Some(status) = error.as_client_api_error().map(|e| e.status_code) {
                println!("HTTP status: {status}");
            }
            return Ok(());
        }
    };
    println!(
        "GET /notifications succeeded: {} notifications, next_token {}",
        response.notifications.len(),
        if response.next_token.is_some() {
            "present"
        } else {
            "absent"
        }
    );
    for notification in &response.notifications {
        let highlight = notification.actions.iter().any(|a| a.is_highlight());
        println!(
            "{} {} highlight={} read={}",
            util::format_ts(notification.ts),
            notification.room_id,
            highlight,
            notification.read
        );
    }
    Ok(())
}
