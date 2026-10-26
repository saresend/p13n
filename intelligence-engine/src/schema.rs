// @generated automatically by Diesel CLI.

diesel::table! {
    session (id) {
        id -> Integer,
        date -> Integer,
        user_id -> Integer,
    }
}

diesel::table! {
    user (id) {
        id -> Integer,
    }
}

diesel::joinable!(session -> user (user_id));

diesel::allow_tables_to_appear_in_same_query!(session, user,);
