// @generated automatically by Diesel CLI.

diesel::table! {
    action (id) {
        id -> Integer,
        source_application -> Text,
        action_name -> Text,
        payload -> Text,
        timestamp -> BigInt,
    }
}

diesel::table! {
    tasks (id) {
        id -> Integer,
        title -> Text,
        status -> Text,
        task_description_path -> Text,
        progress_log_path -> Text,
        model -> Text,
        taken_by_pid -> Integer,
        taken_by_start -> Integer,
    }
}

diesel::allow_tables_to_appear_in_same_query!(action, tasks,);
