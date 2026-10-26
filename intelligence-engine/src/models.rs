use diesel::prelude::*;
use serde_json::Value;

#[derive(Queryable, Selectable)]
#[diesel(table_name=crate::schema::session)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct Session {
    pub id: i32,
    pub date: i32,
    pub user_id: i32,
}

#[derive(Queryable, Selectable, Insertable)]
#[diesel(table_name=crate::schema::user)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct User {
    pub id: i32,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name=crate::schema::action)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct Action {
    pub id: i32,
    pub action_id: String,
    pub payload: String, // should be some json blob
    pub session_id: i32,
}

#[derive(Insertable)]
#[diesel(table_name=crate::schema::session)]
pub(crate) struct NewSession {
    pub user_id: i32,
    pub date: i32,
}

#[derive(Insertable)]
#[diesel(table_name=crate::schema::action)]
pub(crate) struct NewAction<'a, 'b> {
    pub session_id: i32,
    pub action_id: &'a str,
    pub payload: &'b str,
}
