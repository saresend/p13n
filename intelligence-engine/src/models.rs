use diesel::prelude::*;

#[derive(Queryable, Selectable)]
#[diesel(table_name=crate::schema::session)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct Session {
    pub id: i32,
    pub date: i32,
    pub user_id: i32,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name=crate::schema::user)]
#[diesel(check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct User {
    pub id: i32,
}
