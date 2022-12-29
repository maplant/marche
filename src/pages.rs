use std::{collections::HashSet, sync::Arc};

use askama::Template;
use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use chrono::prelude::*;
use futures::{future, stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use thiserror::Error;

use crate::{
    get,
    items::{IncomingOffer, Item, ItemDrop, ItemThumbnail, OutgoingOffer},
    threads::{Post, Reply, Tag, Tags, Thread},
    users::{LevelInfo, ProfileStub, Role, User, UserCache, UserRejection},
};

const THREADS_PER_PAGE: i64 = 25;
const MINUTES_TIMESTAMP_IS_EMPHASIZED: i64 = 60 * 24;

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    offers: usize,
    code:   u16,
    reason: &'static str,
}

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("Not found")]
    NotFound,
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Internal database error: {0}")]
    InternalDbError(#[from] sqlx::Error),
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let status_code = match self {
            ServerError::NotFound => StatusCode::NOT_FOUND,
            ServerError::Unauthorized => StatusCode::UNAUTHORIZED,
            ServerError::InternalDbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status_code,
            ErrorPage {
                offers: 0,
                code:   status_code.as_u16(),
                reason: status_code.canonical_reason().unwrap_or("????"),
            },
        )
            .into_response()
    }
}

#[derive(Debug, Template)]
#[template(path = "items.html")]
pub struct Items {
    offers: usize,
    items:  Vec<ItemStub>,
}

#[derive(Debug)]
pub struct ItemStub {
    id:          i32,
    name:        String,
    description: String,
    item_type:   String,
    attrs:       String,
    thumbnail:   String,
    rarity:      String,
    available:   bool,
}

get!(
    "/items",
    pub async fn items(conn: Extension<PgPool>, user: User) -> Result<Items, ServerError> {
        if user.role != Role::Admin {
            return Err(ServerError::Unauthorized);
        }

        let items = sqlx::query_as("SELECT * FROM items ORDER BY rarity DESC, id DESC, name ASC")
            .fetch(&*conn)
            .filter_map(|item: Result<Item, _>| future::ready(item.ok()))
            .map(|item| ItemStub {
                thumbnail:   item.get_thumbnail_html(rand::random()),
                id:          item.id,
                name:        item.name,
                description: item.description,
                item_type:   serde_json::to_string(&item.item_type).unwrap(),
                attrs:       serde_json::to_string(&item.attributes).unwrap(),
                rarity:      item.rarity.to_string(),
                available:   item.available,
            })
            .collect()
            .await;

        Ok(Items { offers: 0, items })
    }
);

#[derive(Debug, Template)]
#[template(path = "index.html")]
pub struct Index {
    tags:        Vec<Tag>,
    posts:       Vec<ThreadLink>,
    offers:      i64,
    viewer_role: Role,
}

#[derive(Debug, Serialize)]
struct ThreadLink {
    num:            usize,
    id:             i32,
    title:          String,
    date:           String,
    emphasize_date: bool,
    read:           bool,
    jump_to:        i32,
    replies:        String,
    tags:           Vec<String>,
    pinned:         bool,
    locked:         bool,
    hidden:         bool,
}

get! {
    "/",
    pub async fn redirect_to_index() -> Redirect {
        // TODO: Redirect to default language.
        Redirect::to("/t/en")
    }
}

get! {
    "/t/*tags",
    async fn index(
        conn: Extension<PgPool>,
        user: User,
        Path(viewed_tags): Path<String>,
    ) -> Result<Index, Redirect> {
        let viewed_tags = Tags::fetch_from_str(&conn, &*viewed_tags).await;

        // If no tags are selected and the user is not privileged, force
        // the user to redirect to /t/en
        if viewed_tags.is_empty() && user.role < Role::Moderator {
            return Err(Redirect::to("/t/en"));
        }
        let conn = &*conn;
        let user = &user;

        let posts = sqlx::query_as(
            r#"
                SELECT * FROM threads
                WHERE
                    tags @> $1
                ORDER BY
                    pinned DESC,
                    last_post DESC
                LIMIT $2
            "#,
        )
        .bind(viewed_tags.clone().into_ids().collect::<Vec<_>>())
        .bind(THREADS_PER_PAGE)
        .fetch(conn)
        .filter_map(|t: Result<Thread, _>| future::ready(t.ok()))
        .enumerate()
        .then(move |(i, thread)| async move {
            // Format the date:
            // TODO: Consider moving duration->plaintext into common utility
            let duration_since_last_post = Utc::now().naive_utc()
                - Reply::fetch(&conn, thread.last_post)
                    .await?
                    .post_date;
            let duration_min = duration_since_last_post.num_minutes();
            let duration_hours = duration_since_last_post.num_hours();
            let duration_days = duration_since_last_post.num_days();
            let duration_weeks = duration_since_last_post.num_weeks();
            let duration_string: String = if duration_weeks > 0 {
                format!(
                    "{} week{} ago",
                    duration_weeks,
                    if duration_weeks > 1 { "s" } else { "" }
                )
            } else if duration_days > 0 {
                format!(
                    "{} day{} ago",
                    duration_days,
                    if duration_days > 1 { "s" } else { "" }
                )
            } else if duration_hours > 0 {
                format!(
                    "{} hour{} ago",
                    duration_hours,
                    if duration_hours > 1 { "s" } else { "" }
                )
            } else if duration_min >= 5 {
                format!(
                    "{} minute{} ago",
                    duration_min,
                    if duration_min > 1 { "s" } else { "" }
                )
            } else {
                String::from("just now!")
            };

            let replies = match thread.num_replies {
                0 => format!("No replies"),
                1 => format!("1 reply"),
                x => format!("{} replies", x - 1),
            };

            let read = user.has_read(conn, &thread).await?;
            let jump_to = user.next_unread(conn, &thread).await?;

            sqlx::Result::Ok(ThreadLink {
                num: i + 1,
                id: thread.id,
                title: thread.title,
                date: duration_string,
                emphasize_date: duration_min < MINUTES_TIMESTAMP_IS_EMPHASIZED,
                read,
                jump_to,
                replies,
                tags: stream::iter(thread.tags.into_iter())
                    .filter_map(
                        |tid| async move { Tag::fetch_from_id(conn, tid).await.ok().flatten() },
                    )
                    .map(|t| t.name)
                    .collect()
                    .await,
                pinned: thread.pinned,
                locked: thread.locked,
                hidden: thread.hidden,
            })
        })
        .filter_map(|t| future::ready(t.ok()))
        .collect()
        .await;

        Ok(Index {
            tags: viewed_tags.tags,
            posts: posts,
            viewer_role: user.role,
            offers: user.incoming_offers(&*conn).await.unwrap_or(0),
        })
    }
}

#[derive(Template)]
#[template(path = "thread.html")]
pub struct ThreadPage {
    id:          i32,
    title:       String,
    tags:        Vec<String>,
    posts:       Vec<Post>,
    offers:      i64,
    pinned:      bool,
    locked:      bool,
    hidden:      bool,
    viewer_role: Role,
}

get!(
    "/thread/:thread_id",
    async fn view_thread(
        conn: Extension<PgPool>,
        user: User,
        Path(thread_id): Path<i32>,
    ) -> Result<ThreadPage, ServerError> {
        let thread = Thread::fetch_optional(&*conn, thread_id)
            .await?
            .ok_or(ServerError::NotFound)?;

        user.read_thread(&conn, &thread).await?;

        if thread.hidden && user.role == Role::User {
            return Err(ServerError::NotFound);
        }

        let conn = &*conn;
        let user_cache = UserCache::new(conn);
        let posts =
            sqlx::query_as("SELECT * FROM replies WHERE thread_id = $1 ORDER BY post_date ASC")
                .bind(thread_id)
                .fetch(conn)
                .filter_map(|post| async move { post.ok() })
                .then(move |post: Reply| {
                    let user_cache = user_cache.clone();
                    async move {
                        let date = post.post_date.format(crate::DATE_FMT).to_string();
                        let reactions = stream::iter(post.reactions.into_iter())
                            .filter_map(|drop_id| async move {
                                ItemDrop::fetch(conn, drop_id).await.ok()
                            })
                            .filter_map(|item_drop| async move {
                                item_drop.get_thumbnail(conn).await.ok()
                            })
                            .collect()
                            .await;
                        let can_edit = post.author_id == user.id; // TODO: Add time limit for replies
                        let can_react = post.author_id != user.id;
                        let author = user_cache.get(post.author_id).await?;
                        let reward = if let Some(reward) = post.reward {
                            Some(
                                ItemDrop::fetch(conn, reward)
                                    .await?
                                    .get_thumbnail(conn)
                                    .await?,
                            )
                        } else {
                            None
                        };
                        Result::<_, sqlx::Error>::Ok(Post {
                            id: post.id,
                            author,
                            date,
                            reactions,
                            reward,
                            can_edit,
                            can_react,
                            body: post.body,
                            hidden: post.hidden,
                            image: post.image,
                            thumbnail: post.thumbnail,
                            filename: post.filename,
                        })
                    }
                })
                .try_collect()
                .await?;

        Ok(ThreadPage {
            id: thread_id,
            title: thread.title.clone(),
            posts,
            tags: Tags::fetch_from_ids(conn, thread.tags.iter())
                .await
                .into_names()
                .collect(),
            pinned: thread.pinned,
            locked: thread.locked,
            hidden: thread.hidden,
            offers: user.incoming_offers(conn).await?,
            viewer_role: user.role,
        })
    }
);

#[derive(Template, Debug)]
#[template(path = "author.html")]
pub struct AuthorPage {
    offers: i64,
}

get!(
    "/author",
    async fn author_page(conn: Extension<PgPool>, user: User) -> Result<AuthorPage, ServerError> {
        Ok(AuthorPage {
            offers: user.incoming_offers(&*conn).await?,
        })
    }
);

#[derive(Template)]
#[template(path = "item.html")]
pub struct ItemPage {
    id:           i32,
    name:         String,
    description:  String,
    pattern:      u16,
    rarity:       String,
    thumbnail:    String,
    equip_action: Option<AvailableEquipAction>,
    owner_id:     i32,
    owner_name:   String,
    offers:       i64,
}

pub enum AvailableEquipAction {
    Equip,
    Unequip,
}

get!(
    "/item/:drop_id",
    pub async fn show(
        conn: Extension<PgPool>,
        user: User,
        Path(drop_id): Path<i32>,
    ) -> Result<ItemPage, ServerError> {
        let drop = ItemDrop::fetch_optional(&*conn, drop_id)
            .await?
            .ok_or(ServerError::NotFound)?;
        let item = drop.fetch_item(&*conn).await?;
        let owner = User::fetch(&*conn, drop.owner_id).await?;
        let inventory = user.equipped(&*conn).await?;
        let thumbnail = item.get_thumbnail_html(drop.pattern);
        let equip_action = (user.id == drop.owner_id && item.is_equipable()).then(|| {
            if inventory.iter().any(|(_, equipped)| equipped == &drop) {
                AvailableEquipAction::Unequip
            } else {
                AvailableEquipAction::Equip
            }
        });

        Ok(ItemPage {
            thumbnail,
            equip_action,
            id: drop_id,
            name: item.name,
            description: item.description,
            pattern: drop.pattern as u16,
            rarity: item.rarity.to_string(),
            owner_id: owner.id,
            owner_name: owner.name.to_string(),
            offers: user.incoming_offers(&*conn).await?,
        })
    }
);

#[derive(Template)]
#[template(path = "react.html")]
pub struct ReactPage {
    thread_id: i32,
    post_id:   i32,
    author:    ProfileStub,
    body:      String,
    inventory: Vec<ItemThumbnail>,
    offers:    i64,
    image:     Option<String>,
    thumbnail: Option<String>,
    filename:  String,
}

get!(
    "/react/:post_id",
    async fn react_page(
        conn: Extension<PgPool>,
        user: User,
        Path(post_id): Path<i32>,
    ) -> Result<ReactPage, ServerError> {
        let post = Reply::fetch(&*conn, post_id).await?;
        let author = User::fetch(&*conn, post.author_id)
            .await?
            .get_profile_stub(&*conn)
            .await?;

        let inventory: Vec<_> = user
            .inventory(&conn)
            .await?
            .into_iter()
            .filter(|(item, _)| item.is_reaction())
            .map(|(item, drop)| ItemThumbnail::new(&item, &drop))
            .collect();

        Ok(ReactPage {
            thread_id: post.thread_id,
            post_id,
            author,
            body: post.body,
            inventory,
            offers: user.incoming_offers(&*conn).await?,
            image: post.image,
            thumbnail: post.thumbnail,
            filename: post.filename,
        })
    }
);

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginPage {
    offers: usize,
}

#[derive(Deserialize)]
pub struct LoginPageParams {
    redirect: Option<String>,
}

get!(
    "/login",
    async fn login_page(
        user: Result<User, UserRejection>,
        Query(LoginPageParams { redirect }): Query<LoginPageParams>,
    ) -> Result<LoginPage, Redirect> {
        match (redirect, user) {
            (Some(redirect), Ok(_)) => Err(Redirect::to(&redirect)),
            _ => Ok(LoginPage { offers: 0 }),
        }
    }
);

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterPage {
    offers: usize,
}

get! {
    "/register",
    async fn register_page() -> RegisterPage {
        RegisterPage { offers: 0 }
    }
}

#[derive(Template)]
#[template(path = "update_bio.html")]
pub struct UpdateBioPage {
    name:   String,
    bio:    String,
    stub:   ProfileStub,
    offers: usize,
}

get! {
    "/bio",
    async fn update_bio_page(conn: Extension<PgPool>, user: User) -> Result<UpdateBioPage, ServerError> {
        Ok(UpdateBioPage {
            stub:       user.get_profile_stub(&*conn).await?,
            offers:     user.incoming_offers(&*conn).await? as usize,
            name:       user.name,
            bio:        user.bio,
        })
    }
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfilePage {
    bio:           String,
    level:         LevelInfo,
    role:          Role,
    stub:          ProfileStub,
    equipped:      Vec<ItemThumbnail>,
    inventory:     Vec<ItemThumbnail>,
    is_banned:     bool,
    is_curr_user:  bool,
    ban_timestamp: String,
    viewer_role:   Role,
    viewer_name:   String,
    offers:        i64,
    notes:         String,
}

mod filters {
    pub fn redact(input: &str) -> ::askama::Result<String> {
        Ok(input
            .chars()
            .map(|c| {
                if c.is_whitespace() || c == '.' || c == ',' || c == '!' || c == '?' {
                    c
                } else {
                    '█'
                }
            })
            .collect::<String>())
    }
}

get! {
    "/profile",
    async fn show_curr_user_profile(user: User) -> Redirect {
        Redirect::to(&format!("/profile/{}", user.id))
    }
}

get!(
    "/profile/:user_id",
    async fn show_user_profile(
        conn: Extension<PgPool>,
        curr_user: User,
        Path(user_id): Path<i32>,
    ) -> Result<ProfilePage, ServerError> {
        let user = User::fetch_optional(&*conn, user_id)
            .await?
            .ok_or(ServerError::NotFound)?;

        let equipped = user.equipped(&*conn).await?;

        let mut is_equipped = HashSet::new();
        for (_, item_drop) in &equipped {
            is_equipped.insert(item_drop.id);
        }
        let inventory: Vec<_> = user
            .inventory(&*conn)
            .await?
            .into_iter()
            .filter(|(_, item_drop)| !is_equipped.contains(&item_drop.id))
            .map(|(item, item_drop)| ItemThumbnail::new(&item, &item_drop))
            .collect();

        let ban_timestamp = user
            .banned_until
            .map(|x| x.format(crate::DATE_FMT).to_string())
            .unwrap_or_else(String::new);

        Ok(ProfilePage {
            is_banned: user.is_banned(),
            ban_timestamp,
            offers: user.incoming_offers(&*conn).await?,
            stub: user.get_profile_stub(&*conn).await?,
            level: user.level_info(),
            bio: user.bio,
            role: user.role,
            equipped: equipped
                .into_iter()
                .map(|(item, item_drop)| ItemThumbnail::new(&item, &item_drop))
                .collect(),
            inventory,
            is_curr_user: user.id == curr_user.id,
            notes: user.notes,
            viewer_role: curr_user.role,
            viewer_name: curr_user.name,
        })
    }
);

#[derive(Template)]
#[template(path = "leaderboard.html")]
pub struct LeaderboardPage {
    offers: i64,
    users:  Vec<UserRank>,
}

struct UserRank {
    rank: usize,
    bio:  String,
    stub: ProfileStub,
}

get!(
    "/leaderboard",
    async fn show_leaderboard(
        conn: Extension<PgPool>,
        user: User,
    ) -> Result<LeaderboardPage, ServerError> {
        let conn = &*conn;
        let user_profiles =
            sqlx::query_as("SELECT * FROM users ORDER BY experience DESC LIMIT 100")
                .fetch(conn)
                .enumerate()
                .filter_map(|(i, t): (_, Result<User, _>)| future::ready(t.ok().map(|t| (i, t))))
                .then(|(i, u)| async move {
                    sqlx::Result::Ok(UserRank {
                        rank: i + 1,
                        bio:  u.bio.clone(),
                        stub: u.get_profile_stub(conn).await?,
                    })
                })
                .filter_map(|t| future::ready(t.ok()))
                .collect()
                .await;

        Ok(LeaderboardPage {
            users:  user_profiles,
            offers: user.incoming_offers(conn).await?,
        })
    }
);

#[derive(Template)]
#[template(path = "offer.html")]
pub struct TradeRequestPage {
    sender:             ProfileStub,
    sender_inventory:   Vec<ItemThumbnail>,
    receiver:           ProfileStub,
    receiver_inventory: Vec<ItemThumbnail>,
    offers:             i64,
}

get!(
    "/offer/:receiver_id",
    async fn show_offer(
        conn: Extension<PgPool>,
        sender: User,
        Path(receiver_id): Path<i32>,
    ) -> Result<TradeRequestPage, ServerError> {
        let receiver = User::fetch_optional(&*conn, receiver_id)
            .await?
            .ok_or(ServerError::NotFound)?;

        Ok(TradeRequestPage {
            sender:             sender.get_profile_stub(&*conn).await?,
            sender_inventory:   sender
                .inventory(&*conn)
                .await?
                .map(|(i, d)| ItemThumbnail::new(&i, &d))
                .collect(),
            receiver:           receiver.get_profile_stub(&*conn).await?,
            receiver_inventory: receiver
                .inventory(&*conn)
                .await?
                .map(|(i, d)| ItemThumbnail::new(&i, &d))
                .collect(),
            offers:             sender.incoming_offers(&*conn).await?,
        })
    }
);

#[derive(Template)]
#[template(path = "offers.html")]
pub struct TradeRequestsPage {
    user:            ProfileStub,
    incoming_offers: Vec<IncomingOffer>,
    outgoing_offers: Vec<OutgoingOffer>,
    offers:          i64,
}

get!(
    "/offers",
    async fn show_offers(
        conn: Extension<PgPool>,
        user: User,
    ) -> Result<TradeRequestsPage, ServerError> {
        let user_cache = UserCache::new(&*conn);
        let incoming_offers = IncomingOffer::retrieve(&*conn, &user_cache, &user).await;
        let outgoing_offers = OutgoingOffer::retrieve(&*conn, &user_cache, &user).await;

        Ok(TradeRequestsPage {
            user: user.get_profile_stub(&*conn).await?,
            offers: user.incoming_offers(&*conn).await?,
            incoming_offers,
            outgoing_offers,
        })
    }
);
