use crate::schema::{drops, items};
use crate::users::User;
use chrono::NaiveDateTime;
use diesel::prelude::*;
use diesel_derive_enum::DbEnum;
use rand::prelude::{thread_rng, IteratorRandom};
use serde::{Deserialize, Serialize};

/// Rarity of an item.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize, DbEnum)]
pub enum Rarity {
    /// Corresponds to a ~84% chance of being dropped
    Common,

    /// Corresponds to a ~15% chance of being dropped
    Uncommon,

    /// Corresponds to a ~1% chance of being dropped
    Rare,

    /// Corresponds to a ~0.1% chance of being dropped
    UltraRare,

    /// Corresponds to a ~0.01% chance of being dropped
    Legendary,
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

/*
/// The type of an item. Determines if the item has any associated actions or is purely cosmetic,
/// and further if the item is cosmetic how many can be equipped.
///
/// Cosmetic items are effectively associated with a CSS style.
pub enum ItemType {
    /// Cosmetic profile picture, displayable in user profile and next to all posts.
    CosmeticProfilePic,
    /// Cosmetic background, displayed behind the profile.
    CosmeticUserBackground,
    /// Cosmetic profile picture boarder.
    CosmeticBorder,
    /// Cosmetic badge, [MAX_EQUIPABLE] of these can be shown under the pfoile.
    CosmeticBadge,
    /// Cosmetic badge that appears directly under the profile picture. Only one is equipable.
    CosmeticCommendation,
    /// Action item that allows the invitation of a new user.
    ActionInvite,
    /// Post action item that allows to sticky the post.
    PostActionSticky,
    /// Allows for a sticker to be posted on a post.
    Reaction,
}
*/

/// An item that can be dropped.
#[derive(Queryable)]
pub struct Item {
    /// Id of the available item
    pub id: i32,
    /// Name of the item
    pub name: String,
    /// Description of the item
    description: String,
    /// Availability of the item (can the item be dropped?)
    available: bool,
    /// Rarity of the item
    rarity: Rarity,
    /// Link to the action provided by the item
    action_link: String,
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
}

/*
#[derive(Insertable)]
#[table_name = "items"]
pub struct NewItem<'n, 'd> {
    name: &'n str,
    description: &'d str,
    available: bool,
    rarity: Rarity,
    action_link: String,
}
*/

/// A dropped item associated with a user.
#[derive(Queryable)]
pub struct ItemDrop {
    /// Id of the dropped item.
    id: i32,
    /// UserId of the owner.
    owner_id: i32,
    /// ItemId of the item.
    pub item_id: i32,
}

use chrono::{Duration, Utc};

lazy_static::lazy_static! {
    /// The minimum amount of time you are aloud to receive a single drop during.
    static ref DROP_PERIOD: Duration = Duration::minutes(0);
    // static ref DROP_PERIOD: Duration = Duration::days(1);
}

/// Corresponds to a 15% chance to receive a drop.
pub const DROP_CHANCE: u32 = u32::MAX - 644245090;

#[derive(Insertable)]
#[table_name = "drops"]
pub struct NewDrop {
    owner_id: i32,
    item_id: i32,
}

impl ItemDrop {
    pub fn item_id(self) -> i32 {
        self.item_id
    }

    /// Possibly selects an item, depending on the last drop.
    pub fn drop(conn: &PgConnection, user: &User) -> Option<Self> {
        // Determine if we have a drop
        conn.transaction(|| {
            let item: Option<Self> = (user.last_reward < (Utc::now() - *DROP_PERIOD).naive_utc()
                && rand::random::<u32>() > DROP_CHANCE)
                .then(|| {
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
                                })
                                .get_result(conn)
                                .ok()
                        })
                })
                .flatten();

            // Check if the user has had a drop since this time
            if item.is_some() {
                if User::lookup(conn, user.id).unwrap().last_reward != user.last_reward {
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

/// A trade between two users.
#[derive(Queryable)]
pub struct Trade {
    /// Id of the trade.
    id: i32,
    /// UserId of the sender.
    sender: i32,
    /// Items offered for trade (expressed as a vec of OwnedItemIds).
    sender_items: Vec<i32>,
    /// UserId of the receiver.
    receiver: i32,
    /// Items requested for trade
    receiver_items: Vec<i32>,
}
