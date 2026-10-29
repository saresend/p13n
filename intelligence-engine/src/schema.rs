// @generated automatically by Diesel CLI.

diesel::table! {
    action (id) {
        id -> Integer,
        session_id -> Integer,
        action_id -> Text,
        payload -> Text,
    }
}

diesel::table! {
    session (id) {
        id -> Integer,
        date -> Integer,
        user_id -> Integer,
    }
}

diesel::table! {
    suggestions (id) {
        id -> Integer,
        session_id -> Integer,
        sent -> Nullable<Integer>,
        action_id -> Text,
        payload -> Text,
    }
}

diesel::table! {
    user (id) {
        id -> Integer,
    }
}

diesel::joinable!(action -> session (session_id));
diesel::joinable!(session -> user (user_id));
diesel::joinable!(suggestions -> session (session_id));

diesel::allow_tables_to_appear_in_same_query!(action, session, suggestions, user,);
