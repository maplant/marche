table! {
    users (id) {
        id -> Integer,
        name -> Text,
        password -> Text,
        rank_id -> Integer,
    }
}

table! {
    login_sessions(id) {
        id -> Integer,
        user_id -> Integer,
        session_start -> Timestamp,
    }
}

table! {
    threads(id) {
        id -> Integer,
        author_id -> Integer,
        post_date -> Timestamp,
        tile -> Text,
        body -> Text,
    }
}
