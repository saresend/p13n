use std::str::FromStr;

use crate::errors::PError;
use crate::load_db_connection;
use crate::models::{Action, Session, TransportAction};
use crate::schema::action;
use diesel::prelude::*;
use serde_json::Value;
use tokio::sync::broadcast::Sender;

pub(crate) async fn try_generate(
    publisher: Sender<Vec<TransportAction>>,
    session: Session,
) -> Result<(), PError> {
    let mut db_conn = load_db_connection().await;
    let action_set = action::table
        .filter(action::session_id.eq(session.id))
        .load::<Action>(&mut db_conn);

    match action_set {
        Ok(data) => {
            // here we need to invoke the model I assume
            let suggestion = invoke_model(&data).await;
            match suggestion {
                Ok(suggestion) => {
                    // Err only means no one is subscribed right now
                    let _ = publisher.send(suggestion);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(PError::DBError(e)),
    }
}

pub(crate) async fn invoke_model(input: &[Action]) -> Result<Vec<TransportAction>, PError> {
    Ok(input
        .iter()
        .map(|x| TransportAction {
            action_id: x.action_id.clone(),
            payload: Value::from_str(&x.payload).unwrap(),
        })
        .collect())
}
