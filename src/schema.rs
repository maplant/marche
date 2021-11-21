table! {
    users (id) {
        id -> Integer,
        name -> Text,
        password -> Text,
        rank_id -> Integer,
        last_reward -> Timestamp,
    }
}

table! {
    login_sessions(id) {
        id -> Integer,
        session_id -> Varchar,
        user_id -> Integer,
        session_start -> Timestamp,
    }
}

table! {
    threads(id) {
        id -> Integer,
        author_id -> Integer,
        post_date -> Timestamp,
        last_post -> Timestamp,
        title -> Text,
        body -> Text,
        reward -> Nullable<Integer>,
    }
}

table! {
    replies(id) {
        id -> Integer,
        author_id -> Integer,
        thread_id -> Integer,
        post_date -> Timestamp,
        body -> Text,
        reward -> Nullable<Integer>,
    }
}
