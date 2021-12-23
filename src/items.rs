use crate::schema::{drops, trade_requests};
use crate::threads::Reply;
use crate::users::{User, UserCache, UserProfile};
use chrono::Duration;
use diesel::pg::Pg;
use diesel::prelude::*;
use diesel::serialize::Output;
use diesel::sql_types::Jsonb;
use diesel::types::{FromSql, ToSql};
use diesel_derive_enum::DbEnum;
use rand::prelude::{thread_rng, IteratorRandom};
use rocket::form::Form;
use rocket::response::Redirect;
use rocket::uri;
use rocket_dyn_templates::Template;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Rarity of an item.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, DbEnum)]
pub enum Rarity {
    /// Corresponds to a ~84% chance of being dropped:
    Common,
    /// Corresponds to a ~15% chance of being dropped:
    Uncommon,
    /// Corresponds to a ~1% chance of being dropped:
    Rare,
    /// Corresponds to a ~0.1% chance of being dropped:
    UltraRare,
    /// Corresponds to a ~0.01% chance of being dropped:
    Legendary,
    /// Unique items have no chance of being dropped, and must be minted
    Unique,
}

impl ToString for Rarity {
    fn to_string(&self) -> String {
        String::from(match self {
            Self::Common => "common",
            Self::Uncommon => "uncommon",
            Self::Rare => "rare",
            Self::UltraRare => "ultra-rare",
            Self::Legendary => "legendary",
            Self::Unique => "unique",
        })
    }
}

const LEGENDARY: u32 = u32::MAX - 429497;
const ULTRA_RARE: u32 = LEGENDARY - 4294970;
const RARE: u32 = ULTRA_RARE - 42949672;
const UNCOMMON: u32 = RARE - 644245090;

impl Rarity {
    /// Roll for a random rarity
    pub fn roll() -> Self {
        let rng: u32 = rand::random();
        if rng >= LEGENDARY {
            Self::Legendary
        } else if rng >= ULTRA_RARE {
            Self::UltraRare
        } else if rng >= RARE {
            Self::Rare
        } else if rng >= UNCOMMON {
            Self::Uncommon
        } else {
            Self::Common
        }
    }
}

/// The type of an item. Determines if the item has any associated actions or is purely cosmetic,
/// and further if the item is cosmetic how many can be equipped
#[derive(Clone, Debug, Serialize, Deserialize, FromSqlRow, AsExpression)]
#[sql_type = "Jsonb"]
pub enum ItemType {
    /// An item with no use
    Useless,
    /// Cosmetic profile picture, displayable in user profile and next to all posts
    Avatar { filename: String },
    /// Cosmetic background, displayed behind the profile
    ProfileBackground { colors: Vec<String> },
    /// Reaction image, consumable as an attachment to posts
    Reaction { filename: String },
}

impl ToSql<Jsonb, Pg> for ItemType {
    fn to_sql<W: std::io::Write>(&self, out: &mut Output<'_, W, Pg>) -> diesel::serialize::Result {
        out.write_all(&[1])?;
        serde_json::to_writer(out, self)
            .map(|_| diesel::serialize::IsNull::No)
            .map_err(Into::into)
    }
}

impl FromSql<Jsonb, Pg> for ItemType {
    fn from_sql(bytes: Option<&[u8]>) -> diesel::deserialize::Result<Self> {
        serde_json::from_slice(&bytes.unwrap_or(&[])[1..]).map_err(|_| "Invalid Json".into())
    }
}

/// An item that can be dropped
#[derive(Queryable, Debug)]
pub struct Item {
    /// Id of the available item
    pub id: i32,
    /// Name of the item
    pub name: String,
    /// Description of the item
    pub description: String,
    /// Availability of the item (can the item be dropped?)
    pub available: bool,
    /// Rarity of the item
    pub rarity: Rarity,
    /// Type of the item
    #[diesel(sql_type = "ItemType")]
    pub item_type: ItemType,
}

impl Item {
    pub fn fetch(conn: &PgConnection, item_id: i32) -> Self {
        use crate::schema::items::dsl::*;
        items
            .filter(id.eq(item_id))
            .load::<Self>(conn)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    pub fn is_reaction(&self) -> bool {
        matches!(self.item_type, ItemType::Reaction { .. })
    }
}

/// A dropped item associated with a user
#[derive(Queryable, Debug)]
pub struct ItemDrop {
    /// Id of the dropped item
    pub id: i32,
    /// UserId of the owner
    pub owner_id: i32,
    /// ItemId of the item
    pub item_id: i32,
    /// Unique pattern Id for the item
    pub pattern: i16,
    /// Indicates if the drop has been consumed
    pub consumed: bool,
}

lazy_static::lazy_static! {
    /// The minimum amount of time you are aloud to receive a single drop during.
    static ref DROP_PERIOD: Duration = Duration::minutes(0);
    // static ref DROP_PERIOD: Duration = Duration::days(1);
}

/// Corresponds to a 15% chance to receive a drop.
// pub const DROP_CHANCE: u32 = u32::MAX - 644245090;
/// Four in five chance for drop.
pub const DROP_CHANCE: u32 = u32::MAX / 5;

#[derive(Insertable)]
#[table_name = "drops"]
pub struct NewDrop {
    owner_id: i32,
    item_id: i32,
    pattern: i16,
    consumed: bool,
}

impl ItemDrop {
    pub fn item_id(self) -> i32 {
        self.item_id
    }

    pub fn fetch(conn: &PgConnection, drop_id: i32) -> Self {
        use crate::schema::drops::dsl::*;
        drops.filter(id.eq(drop_id)).first::<Self>(conn).unwrap()
    }

    pub fn thumbnail_html(&self, conn: &PgConnection) -> String {
        let item = Item::fetch(&conn, self.item_id);
        match item.item_type {
            ItemType::Useless => String::from(r#"<div class="fixed-item-thumbnail">?</div>"#),
            ItemType::Avatar { filename } => format!(
                r#"<img src="/static/{}.png" style="width: 50px; height: auto;">"#,
                filename
            ),
            ItemType::ProfileBackground { .. } => {
                // This is kind of redundant (we fetch Item twice), whatever
                format!(
                    r#"<div class="fixed-item-thumbnail" style="{}"></div>"#,
                    self.background_style(conn)
                )
            }
            ItemType::Reaction { filename } => format!(
                // TODO(map): Add rotation
                r#"<img src="/static/{}.png" style="width: 50px; height: auto; transform: rotate({}deg);">"#,
                filename,
                self.rotation(),
            ),
        }
    }

    // TODO: Get rid of this in favor of something better.
    pub fn thumbnail(&self, conn: &PgConnection) -> ItemThumbnail {
        let item = Item::fetch(&conn, self.item_id);
        ItemThumbnail {
            id: self.id,
            name: item.name.clone(),
            rarity: item.rarity.to_string(),
            thumbnail: self.thumbnail_html(conn),
        }
    }

    pub fn profile_pic(&self, conn: &PgConnection) -> String {
        let item = Item::fetch(&conn, self.item_id);
        match item.item_type {
            ItemType::Avatar { filename } => filename,
            _ => panic!("Item is not a profile picture"),
        }
    }

    pub fn background_style(&self, conn: &PgConnection) -> String {
        let item = Item::fetch(&conn, self.item_id);
        match item.item_type {
            ItemType::ProfileBackground { colors } => {
                let mut style = format!(
                    r#"background: linear-gradient({}deg"#,
                    // Convert patten to unsigned integer and then convert to a
                    // degree value.
                    (self.pattern as u16) as f32 / (u16::MAX as f32) * 360.0,
                );
                for color in colors {
                    style += ", ";
                    style += &color;
                }
                style += ");";
                style
            }
            _ => panic!("Item is not a profile picture"),
        }
    }

    pub fn rotation(&self) -> f32 {
        (self.pattern as u16) as f32 / (u16::MAX as f32) * 360.0
    }

    /// Possibly selects an item, depending on the last drop.
    pub fn drop(conn: &PgConnection, user: &User) -> Option<Self> {
        // Determine if we have a drop
        conn.transaction(|| {
            let item: Option<Self> =
                // (user.last_reward < (Utc::now() - *DROP_PERIOD).naive_utc() && 
                (rand::random::<u32>() > DROP_CHANCE).then(|| {
                    use crate::schema::items::dsl::*;

                    // If we have a drop, select a random rarity.
                    let rolled = Rarity::roll();

                    // Query available items from the given rarity and randomly choose one.
                    items
                        .filter(rarity.eq(rolled))
                        .filter(available.eq(true))
                        .load::<Item>(conn)
                        .ok()
                        .unwrap_or_else(Vec::new)
                        .into_iter()
                        .choose(&mut thread_rng())
                        .and_then(|chosen| {
                            // Give the new item to the user
                            diesel::insert_into(drops::table)
                                .values(NewDrop {
                                    owner_id: user.id,
                                    item_id: chosen.id,
                                    pattern: rand::random(),
                                    consumed: false,
                                })
                                .get_result(conn)
                                .ok()
                        })
                })
                .flatten();

            // Check if the user has had a drop since this time
            if item.is_some() {
                if User::fetch(conn, user.id).unwrap().last_reward != user.last_reward {
                    // Rollback the transaction
                    Err(diesel::result::Error::RollbackTransaction)
                } else {
                    // Otherwise, attempt to set a new last drop.
                    user.update_last_reward(conn)
                        .map_err(|_| diesel::result::Error::RollbackTransaction)
                        .map(move |_| item)
                }
            } else {
                Ok(item)
            }
        })
        .ok()
        .flatten()
    }
}

// TODO: Take this struct and extract it somewhere
#[derive(Serialize)]
pub struct ItemThumbnail {
    pub id: i32,
    pub name: String,
    pub rarity: String,
    pub thumbnail: String,
}

#[rocket::get("/item/<drop_id>")]
pub fn item(user: User, drop_id: i32) -> Template {
    #[derive(Serialize)]
    struct Context {
        id: i32,
        name: String,
        description: String,
        pattern: u16,
        rarity: String,
        thumbnail: String,
        can_equip: bool,
    }

    let conn = crate::establish_db_connection();
    let drop = ItemDrop::fetch(&conn, drop_id);
    let item = Item::fetch(&conn, drop.item_id);

    Template::render(
        "item",
        Context {
            id: drop_id,
            name: item.name,
            description: item.description,
            pattern: drop.pattern as u16,
            rarity: item.rarity.to_string(),
            thumbnail: drop.thumbnail_html(&conn),
            can_equip: user.id == drop.owner_id,
        },
    )
}

#[rocket::get("/react/<post_id>")]
pub fn react(user: User, post_id: i32) -> Template {
    #[derive(Serialize)]
    struct Context<'p> {
        post_id: i32,
        author: UserProfile,
        body: &'p str,
        inventory: Vec<ItemThumbnail>,
    }

    let conn = crate::establish_db_connection();
    let post = Reply::fetch(&conn, post_id);
    let author = User::fetch(&conn, post.author_id).unwrap().profile(&conn);
    let inventory: Vec<_> = user
        .inventory(&conn)
        .filter_map(|(item, drop)| item.is_reaction().then(|| drop.thumbnail(&conn)))
        .collect();

    Template::render(
        "react",
        Context {
            post_id,
            author,
            body: &post.body,
            inventory,
        },
    )
}

#[rocket::post("/react/<post_id>", data = "<used_reactions>")]
pub fn react_action(
    user: User,
    post_id: i32,
    used_reactions: Form<HashMap<i32, bool>>,
) -> Redirect {
    let conn = crate::establish_db_connection();
    let thread_id = Reply::fetch(&conn, post_id).thread_id;

    let _ = conn.transaction(|| -> Result<(), diesel::result::Error> {
        use crate::schema::replies::dsl::*;
        use diesel::result::Error;

        // Get the reply
        let reply = replies
            .filter(id.eq(post_id))
            .first::<Reply>(&conn)
            .map_err(|_| Error::RollbackTransaction)?;

        let mut new_reactions = reply.reactions;

        // Verify that the author does not own this reply:
        if reply.author_id == user.id {
            return Err(Error::RollbackTransaction);
        }

        // Verify that all of the reactions are owned by the user:
        for (&reaction, selected) in &*used_reactions {
            let drop = ItemDrop::fetch(&conn, reaction);
            let item = Item::fetch(&conn, drop.item_id);
            if !selected || drop.owner_id != user.id || !item.is_reaction() {
                return Err(Error::RollbackTransaction);
            }

            // Set the drops to consumed.
            use crate::schema::drops::dsl::*;
            diesel::update(drops.find(reaction))
                .filter(consumed.eq(false))
                .set(consumed.eq(true))
                .get_result::<ItemDrop>(&conn)
                .map_err(|_| Error::RollbackTransaction)?;

            new_reactions.push(reaction);
        }

        // Update the post with the new reactions:
        diesel::update(replies.find(post_id))
            .set(reactions.eq(new_reactions))
            .get_result::<Reply>(&conn)
            .map_err(|_| Error::RollbackTransaction)?;

        Ok(())
    });

    Redirect::to(format!(
        "{}#{}",
        uri!(crate::threads::thread(thread_id)),
        post_id
    ))
}

/// A trade between two users.
#[derive(Queryable)]
pub struct TradeRequest {
    /// Id of the trade.
    pub id: i32,
    /// UserId of the sender.
    pub sender_id: i32,
    /// Items offered for trade (expressed as a vec of OwnedItemIds).
    pub sender_items: Vec<i32>,
    /// UserId of the receiver.
    pub receiver_id: i32,
    /// Items requested for trade
    pub receiver_items: Vec<i32>,
}

impl TradeRequest {
    pub fn fetch(conn: &PgConnection, req_id: i32) -> Self {
        use crate::schema::trade_requests::dsl::*;

        trade_requests
            .filter(id.eq(req_id))
            .load::<Self>(conn)
            .ok()
            .and_then(|v| v.into_iter().next())
            .unwrap()
    }

    pub fn accept(&self, conn: &PgConnection) {
        use crate::schema::drops::dsl::*;

        let res = conn.transaction(|| -> Result<(), diesel::result::Error> {
            let sender = User::fetch(conn, self.sender_id).unwrap();
            let receiver = User::fetch(conn, self.receiver_id).unwrap();

            for sender_item in &self.sender_items {
                sender.unequip(
                    conn,
                    diesel::update(drops.find(sender_item))
                        .set(owner_id.eq(self.receiver_id))
                        .filter(owner_id.eq(self.sender_id))
                        .get_result::<ItemDrop>(conn)
                        .map_err(|_| diesel::result::Error::RollbackTransaction)?,
                );
            }
            for receiver_item in &self.receiver_items {
                receiver.unequip(
                    conn,
                    diesel::update(drops.find(receiver_item))
                        .set(owner_id.eq(self.sender_id))
                        .filter(owner_id.eq(self.receiver_id))
                        .get_result::<ItemDrop>(conn)
                        .map_err(|_| diesel::result::Error::RollbackTransaction)?,
                );
            }

            // Check if any item no longer belongs to their respective owner:
            for sender_item in &self.sender_items {
                if drops
                    .filter(id.eq(sender_item))
                    .filter(consumed.eq(false)) // Consumed items may not be traded
                    .first::<ItemDrop>(conn)
                    .map_err(|_| diesel::result::Error::RollbackTransaction)?
                    .owner_id
                    != self.receiver_id
                {
                    return Err(diesel::result::Error::RollbackTransaction);
                }
            }
            for receiver_item in &self.receiver_items {
                if drops
                    .filter(id.eq(receiver_item))
                    .filter(consumed.eq(false))
                    .first::<ItemDrop>(conn)
                    .map_err(|_| diesel::result::Error::RollbackTransaction)?
                    .owner_id
                    != self.sender_id
                {
                    return Err(diesel::result::Error::RollbackTransaction);
                }
            }
            // delete the transaction
            self.decline(&conn);
            Ok(())
        });

        // Decline the request if we got an error
        if res.is_err() {
            self.decline(&conn);
        }
    }

    pub fn decline(&self, conn: &PgConnection) {
        use crate::schema::trade_requests::dsl::*;

        diesel::delete(trade_requests.filter(id.eq(self.id)))
            .execute(conn)
            .unwrap();
    }
}

#[derive(Insertable)]
#[table_name = "trade_requests"]
pub struct NewTradeRequest {
    sender_id: i32,
    sender_items: Vec<i32>,
    receiver_id: i32,
    receiver_items: Vec<i32>,
}

#[rocket::get("/offer/<receiver_id>")]
pub fn offer(sender: User, receiver_id: i32) -> Template {
    #[derive(Serialize)]
    struct Context {
        sender: UserProfile,
        sender_inventory: Vec<ItemThumbnail>,
        receiver: UserProfile,
        receiver_inventory: Vec<ItemThumbnail>,
    }

    let conn = crate::establish_db_connection();
    let receiver = User::fetch(&conn, receiver_id).unwrap();

    Template::render(
        "offer",
        Context {
            sender: sender.profile(&conn),
            // Got to put this somewhere, but don't know where
            sender_inventory: sender
                .inventory(&conn)
                .map(|(_, d)| d.thumbnail(&conn))
                .collect(),
            receiver: receiver.profile(&conn),
            receiver_inventory: receiver
                .inventory(&conn)
                .map(|(_, d)| d.thumbnail(&conn))
                .collect(),
        },
    )
}

#[rocket::post("/offer/<receiver_id>", data = "<trade>")]
pub fn offer_action(sender: User, receiver_id: i32, trade: Form<HashMap<i32, i32>>) -> Redirect {
    let mut sender_items = Vec::new();
    let mut receiver_items = Vec::new();

    let conn = crate::establish_db_connection();

    for (&item, &trader) in trade.iter() {
        let drop = ItemDrop::fetch(&conn, item);
        if trader != drop.owner_id {
            return Redirect::to(uri!(crate::threads::index()));
        }
        if trader == sender.id {
            sender_items.push(drop.id);
        } else {
            receiver_items.push(drop.id);
        }
    }

    let _: Result<TradeRequest, _> = diesel::insert_into(trade_requests::table)
        .values(&NewTradeRequest {
            sender_id: sender.id,
            sender_items,
            receiver_id,
            receiver_items,
        })
        .get_result(&conn);

    Redirect::to(uri!(offers()))
}

#[rocket::get("/decline/<trade_id>")]
pub fn decline(user: User, trade_id: i32) -> Redirect {
    let conn = crate::establish_db_connection();
    let req = TradeRequest::fetch(&conn, trade_id);
    if req.sender_id == user.id || req.receiver_id == user.id {
        req.decline(&conn);
    }
    Redirect::to(uri!(offers()))
}

#[rocket::get("/accept/<trade_id>")]
pub fn accept(user: User, trade_id: i32) -> Redirect {
    let conn = crate::establish_db_connection();
    let req = TradeRequest::fetch(&conn, trade_id);
    if req.sender_id == user.id || req.receiver_id == user.id {
        req.accept(&conn);
    }
    Redirect::to(uri!(offers()))
}

#[rocket::get("/offers")]
pub fn offers(user: User) -> Template {
    use crate::schema::trade_requests::dsl::*;

    #[derive(Serialize)]
    struct InOffer {
        id: i32,
        sender: UserProfile,
        sender_items: Vec<ItemThumbnail>,
        receiver_items: Vec<ItemThumbnail>,
    }

    #[derive(Serialize)]
    struct OutOffer {
        id: i32,
        sender_items: Vec<ItemThumbnail>,
        receiver: UserProfile,
        receiver_items: Vec<ItemThumbnail>,
    }

    #[derive(Serialize)]
    struct Context {
        user: UserProfile,
        incoming_offers: Vec<InOffer>,
        outgoing_offers: Vec<OutOffer>,
    }

    let conn = crate::establish_db_connection();
    let mut user_cache = UserCache::new(&conn);
    // TODO: filter out trade requests that are no longer valid.
    let incoming_offers: Vec<_> = trade_requests
        .filter(receiver_id.eq(user.id))
        .load::<TradeRequest>(&conn)
        .unwrap()
        .into_iter()
        .map(|trade| -> InOffer {
            InOffer {
                id: trade.id,
                sender: user_cache.get(trade.sender_id).clone(),
                sender_items: trade
                    .sender_items
                    .into_iter()
                    .map(|i| ItemDrop::fetch(&conn, i).thumbnail(&conn))
                    .collect(),
                receiver_items: trade
                    .receiver_items
                    .into_iter()
                    .map(|i| ItemDrop::fetch(&conn, i).thumbnail(&conn))
                    .collect(),
            }
        })
        .collect();
    let outgoing_offers: Vec<_> = trade_requests
        .filter(sender_id.eq(user.id))
        .load::<TradeRequest>(&conn)
        .unwrap()
        .into_iter()
        .map(|trade| -> OutOffer {
            OutOffer {
                id: trade.id,
                receiver: user_cache.get(trade.receiver_id).clone(),
                sender_items: trade
                    .sender_items
                    .into_iter()
                    .map(|i| ItemDrop::fetch(&conn, i).thumbnail(&conn))
                    .collect(),
                receiver_items: trade
                    .receiver_items
                    .into_iter()
                    .map(|i| ItemDrop::fetch(&conn, i).thumbnail(&conn))
                    .collect(),
            }
        })
        .collect();

    Template::render(
        "offers",
        Context {
            user: user.profile(&conn),
            incoming_offers,
            outgoing_offers,
        },
    )
}
