table! {
    users (id) {
        id -> Integer,
        name -> Text,
        password -> Text,
        bio -> Text,
        rank_id -> Integer,
        last_reward -> Timestamp,
        equip_slot_prof_pic -> Nullable<Integer>,
        equip_slot_background -> Nullable<Integer>,
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
        last_post -> Timestamp,
        title -> Text,
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

table! {
    use diesel::sql_types::*;
    use crate::items::RarityMapping;

    items(id) {
        id -> Integer,
        name -> Text,
        description -> Text,
        available -> Bool,
        rarity -> RarityMapping,
        item_type -> Jsonb,
    }
}

table! {
    drops(id) {
        id -> Integer,
        owner_id -> Integer,
        item_id -> Integer,
        pattern -> SmallInt,
    }
}

table! {
    trade_requests(id) {
        id -> Integer,
        sender_id -> Integer,
        sender_items -> Array<Integer>,
        receiver_id -> Integer,
        receiver_items -> Array<Integer>,
    }
}
