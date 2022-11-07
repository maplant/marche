use std::collections::HashSet;

use askama::Template;
use axum::{
    extract::{Extension, Path},
    response::Redirect,
};
use chrono::prelude::*;
use futures::{future, stream, StreamExt};
use serde::Serialize;
use sqlx::PgPool;

use crate::{
    get,
    items::{/*IncomingOffer,*/ Item, ItemDrop /*ItemThumbnail, OutgoingOffer*/},
    threads::{Reply, Tag, Tags, Thread},
    users::{LevelInfo, ProfileStub, Role, User /*, UserCache*/},
    NotFound,
};

const THREADS_PER_PAGE: i64 = 25;
const MINUTES_TIMESTAMP_IS_EMPHASIZED: i64 = 60 * 24;

#[derive(Template)]
#[template(path = "index.html")]
pub struct Index {
    tags: Vec<Tag>,
    posts: Vec<ThreadLink>,
    offers: i64,
    viewer_role: Role,
}

#[derive(Serialize)]
struct ThreadLink {
    num: usize,
    id: i32,
    title: String,
    date: String,
    emphasize_date: bool,
    read: bool,
    jump_to: i32,
    replies: String,
    tags: Vec<String>,
    pinned: bool,
    locked: bool,
    hidden: bool,
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
                    .unwrap()
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

            Result::<_, sqlx::Error>::Ok(ThreadLink {
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
            offers: 0, // IncomingOffer::count(&conn, &user),
        })
    }
}

/*
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

#[derive(Serialize)]
struct Reward {
    thumbnail:   String,
    name:        String,
    description: String,
    rarity:      String,
}

#[derive(Serialize)]
struct Post {
    id:        i32,
    author:    ProfileStub,
    body:      String,
    body_html: String,
    date:      String,
    reactions: Vec<ItemThumbnail>,
    reward:    Option<Reward>,
    can_react: bool,
    can_edit:  bool,
    hidden:    bool,
    image:     Option<String>,
    thumbnail: Option<String>,
    filename:  String,
}

get! {
    "/thread/:thread_id",
    async fn view_thread(
        pool: Extension<PgPool>,
        user: User,
        Path(view_thread_id): Path<i32>
    ) -> Result<ThreadPage, NotFound> {
        use crate::schema::replies::dsl::*;

        let conn = &mut pool.get().expect("Could not connect to db");
        let offers = user.incoming_offers(&conn);
        let thread = crate::schema::threads::table.find(thread_id)
            .first::<Thread>(conn)
            .map_err(|_| NotFound::new(offers))?;
        user.read_thread(&conn, &thread);

        if thread.hidden && user.role == Role::User {
            return Err(NotFound::new(offers));
        }

        let mut user_cache = UserCache::new(&conn);
        let posts = replies
            .filter(thread_id.eq(view_thread_id))
            .order(post_date.asc())
            .load::<Reply>(conn)
            .unwrap()
            .into_iter()
            .map(|t| Post {
                id:        t.id,
                author:    user_cache.get(t.author_id).clone(),
                body:      t.body,
                body_html: t.body_html,
                // TODO: we need to add a user setting to format this to the local time.
                date:      t.post_date.format(crate::DATE_FMT).to_string(),
                reactions: t
                    .reactions
                    .into_iter()
                    .map(|d| ItemDrop::fetch(&conn, d).unwrap().thumbnail(&conn))
                    .collect(),
                reward:    t.reward.map(|r| {
                    let drop = ItemDrop::fetch(&conn, r).unwrap();
                    let item = drop.fetch_item(&conn);
                    Reward {
                        name:        item.name,
                        description: item.description,
                        thumbnail:   drop.thumbnail_html(&conn),
                        rarity:      item.rarity.to_string(),
                    }
                }),
                can_edit:  t.author_id == user.id, // TODO: Add time limit for replies
                can_react: t.author_id != user.id,
                hidden:    t.hidden,
                image:     t.image,
                thumbnail: t.thumbnail,
                filename:  t.filename,
            })
            .collect::<Vec<_>>();

        Ok(ThreadPage {
            id: view_thread_id,
            title: thread.title.clone(),
            posts,
            tags: Tags::fetch_from_ids(&conn, thread.tags.iter()).into_names().collect(),
            pinned: thread.pinned,
            locked: thread.locked,
            hidden: thread.hidden,
            offers: IncomingOffer::count(&conn, &user),
            viewer_role: user.role,
        })
    }
}

#[derive(Template, Debug)]
#[template(path = "author.html")]
pub struct AuthorPage {
    offers: i64,
}

get! {
    "/author",
    async fn author_page(pool: Extension<PgPool>, user: User) -> AuthorPage {
        AuthorPage {
            offers: IncomingOffer::count(&pool.get().expect("Could not connect to db"), &user),
        }
    }
}

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

get! {
    "/item/:drop_id",
    pub async fn show(
        pool: Extension<PgPool>,
        user: User, Path(drop_id):
        Path<i32>
    ) -> Result<ItemPage, NotFound> {
        let conn = pool.get().expect("Could not connect to db");
        // TODO: Fix NotFound
        let drop = ItemDrop::fetch(&conn, drop_id).map_err(|_| NotFound::new(0))?;
        let item = Item::fetch(&conn, drop.item_id);
        let owner = User::fetch(&conn, drop.owner_id).unwrap();
        let equip_action = (user.id == drop.owner_id && item.is_equipable()).then(|| {
            if drop.is_equipped(&conn) {
                AvailableEquipAction::Unequip
            } else {
                AvailableEquipAction::Equip
            }
        });

        Ok(ItemPage {
            id: drop_id,
            name: item.name,
            description: item.description,
            pattern: drop.pattern as u16,
            rarity: item.rarity.to_string(),
            thumbnail: drop.thumbnail_html(&conn),
            owner_id: owner.id,
            owner_name: owner.name.to_string(),
            equip_action,
            offers: IncomingOffer::count(&conn, &user),
        })
    }
}

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

get! {
    "/react/:post_id",
    async fn react_page(
        pool: Extension<PgPool>,
        user: User,
        Path(post_id): Path<i32>
    ) -> ReactPage {
        let conn = pool.get().expect("Could not connect to db");
        let post = Reply::fetch(&conn, post_id).unwrap();
        let author = User::fetch(&conn, post.author_id)
            .unwrap()
            .profile_stub(&conn);
        let inventory: Vec<_> = user
            .inventory(&conn)
            .filter_map(|(item, drop)| item.is_reaction().then(|| drop.thumbnail(&conn)))
            .collect();

        ReactPage {
            thread_id: post.thread_id,
            post_id,
            author,
            body: post.body,
            inventory,
            offers: IncomingOffer::count(&conn, &user),
            image: post.image,
            thumbnail: post.thumbnail,
            filename: post.filename,
        }
    }
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginPage {
    offers: usize,
}

get! {
    "/login",
    async fn login_page() -> LoginPage {
        LoginPage { offers: 0 }
    }
}

#[derive(Template)]
#[template(path = "update_bio.html")]
pub struct UpdateBioPage {
    name:       String,
    picture:    Option<String>,
    badges:     Vec<String>,
    background: String,
    bio:        String,
    offers:     usize,
}

get! {
    "/bio",
    async fn update_bio_page(pool: Extension<PgPool>, user: User) -> UpdateBioPage {
        let conn = pool.get().expect("Could not connect to db");
        UpdateBioPage {
            picture:    user.get_profile_pic(&conn),
            badges:     user.get_badges(&conn),
            background: user.get_background_style(&conn),
            offers:     user.incoming_offers(&conn) as usize,
            bio:        user.bio,
            name:       user.name,
        }
    }
}

#[derive(Template)]
#[template(path = "profile.html")]
pub struct ProfilePage {
    id:            i32,
    name:          String,
    picture:       Option<String>,
    bio:           String,
    level:         LevelInfo,
    role:          Role,
    equipped:      Vec<ItemThumbnail>,
    inventory:     Vec<ItemThumbnail>,
    badges:        Vec<String>,
    background:    String,
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

get! {
    "/profile/:user_id",
    async fn show_user_profile(
        pool: Extension<PgPool>,
        curr_user: User,
        Path(user_id): Path<i32>
    ) -> Result<ProfilePage, NotFound> {
        use crate::schema::drops;

        let conn = pool.get().expect("Could not connect to db");
        let user =
            User::fetch(&conn, user_id).map_err(|_| NotFound::new(curr_user.incoming_offers(&conn)))?;

        let mut is_equipped = HashSet::new();

        let equipped = user
            .equipped(&conn)
            .unwrap()
            .into_iter()
            .map(|drop| {
                // TODO: extract this to function.
                is_equipped.insert(drop.id);
                drop.thumbnail(&conn)
            })
            .collect::<Vec<_>>();

        let mut inventory = drops::table
            .filter(drops::dsl::owner_id.eq(user.id))
            .filter(drops::dsl::consumed.eq(false))
            .load::<ItemDrop>(&conn)
            .ok()
            .unwrap_or_else(Vec::new)
            .into_iter()
            .filter_map(|drop| {
                (!is_equipped.contains(&drop.id)).then(|| {
                    let id = drop.item_id;
                    (drop, Item::fetch(&conn, id).rarity)
                })
            })
            .collect::<Vec<_>>();

        inventory.sort_by(|a, b| a.1.cmp(&b.1).reverse());

        let inventory = inventory
            .into_iter()
            .map(|(drop, _)| drop.thumbnail(&conn))
            .collect::<Vec<_>>();

        let ban_timestamp = user
            .banned_until
            .map(|x| x.format(crate::DATE_FMT).to_string())
            .unwrap_or_else(String::new);

        Ok(ProfilePage {
            id: user_id,
            is_banned: user.is_banned(),
            ban_timestamp,
            picture: user.get_profile_pic(&conn),
            offers: crate::items::IncomingOffer::count(&conn, &curr_user),
            level: user.level_info(),
            badges: user.get_badges(&conn),
            background: user.get_background_style(&conn),
            name: user.name,
            bio: user.bio,
            role: user.role,
            equipped,
            inventory,
            is_curr_user: user.id == curr_user.id,
            notes: user.notes,
            viewer_role: curr_user.role,
            viewer_name: curr_user.name,
        })
    }
}

#[derive(Template)]
#[template(path = "leaderboard.html")]
pub struct LeaderboardPage {
    offers: i64,
    users:  Vec<UserRank>,
}

struct UserRank {
    rank:    usize,
    bio:     String,
    profile: ProfileStub,
}

get! {
    "/leaderboard",
    async fn show_leaderboard(
        pool: Extension<PgPool>,
        user: User
    ) -> LeaderboardPage {
        use crate::schema::users::dsl::*;

        let conn = pool.get().expect("Could not connect to db");

        let user_profiles = users
            .order(experience.desc())
            .limit(100)
            .load::<User>(&conn)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(i, u)| UserRank {
                rank:    i + 1,
                bio:     u.bio.clone(),
                profile: u.profile_stub(&conn),
            })
            .collect();

        LeaderboardPage {
            users:  user_profiles,
            offers: IncomingOffer::count(&conn, &user),
        }
    }
}

#[derive(Template)]
#[template(path = "offer.html")]
pub struct TradeRequestPage {
    sender:             ProfileStub,
    sender_inventory:   Vec<ItemThumbnail>,
    receiver:           ProfileStub,
    receiver_inventory: Vec<ItemThumbnail>,
    offers:             i64,
}

get! {
    "/offer/:receiver_id",
    async fn show_offer(
        pool: Extension<PgPool>,
        sender: User,
        Path(receiver_id): Path<i32>
    ) -> TradeRequestPage {
        let conn = pool.get().expect("Could not connect to db");
        let receiver = User::fetch(&conn, receiver_id).unwrap();

        TradeRequestPage {
            sender:             sender.profile_stub(&conn),
            // Got to put this somewhere, but don't know where
            sender_inventory:   sender
                .inventory(&conn)
                .map(|(_, d)| d.thumbnail(&conn))
                .collect(),
            receiver:           receiver.profile_stub(&conn),
            receiver_inventory: receiver
                .inventory(&conn)
                .map(|(_, d)| d.thumbnail(&conn))
                .collect(),
            offers:             IncomingOffer::count(&conn, &sender),
        }
    }
}

#[derive(Template)]
#[template(path = "offers.html")]
pub struct TradeRequestsPage {
    user:            ProfileStub,
    incoming_offers: Vec<IncomingOffer>,
    outgoing_offers: Vec<OutgoingOffer>,
    offers:          i64,
}

get! {
    "/offers/",
    async fn show_offers(
        pool: Extension<PgPool>,
        user: User
    ) -> TradeRequestsPage {
        let conn = pool.get().expect("Could not connect to db");
        let mut user_cache = UserCache::new(&conn);
        // TODO: filter out trade requests that are no longer valid.
        let incoming_offers: Vec<_> = IncomingOffer::retrieve(&conn, &mut user_cache, &user);
        let outgoing_offers: Vec<_> = OutgoingOffer::retrieve(&conn, &mut user_cache, &user);

        TradeRequestsPage {
            user: user.profile_stub(&conn),
            offers: incoming_offers.len() as i64,
            incoming_offers,
            outgoing_offers,
        }
    }
}
*/
