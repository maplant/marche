use crate::schema::drops;
use crate::users::User;
use diesel::pg::Pg;
use diesel::prelude::*;
use diesel::serialize::Output;
use diesel::sql_types::Jsonb;
use diesel::types::{FromSql, ToSql};
use diesel_derive_enum::DbEnum;
use rand::prelude::{thread_rng, IteratorRandom};
use rocket_dyn_templates::Template;
use serde::{Deserialize, Serialize};

/// Rarity of an item.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, DbEnum)]
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
    ProfilePic { filename: String },
    /// Cosmetic background, displayed behind the profile
    ProfileBackground { colors: Vec<String> },
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
#[derive(Queryable)]
pub struct Item {
    /// Id of the available item
    pub id: i32,
    /// Name of the item
    pub name: String,
    /// Description of the item
    pub description: String,
    /// Thumbnail of the item when viewed in inventory
    pub thumbnail: String,
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
}

/// A dropped item associated with a user
#[derive(Queryable)]
pub struct ItemDrop {
    /// Id of the dropped item
    pub id: i32,
    /// UserId of the owner
    pub owner_id: i32,
    /// ItemId of the item
    pub item_id: i32,
    /// Unique pattern Id for the item
    pub pattern: i16,
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
    pattern: i16,
}

impl ItemDrop {
    pub fn item_id(self) -> i32 {
        self.item_id
    }

    pub fn fetch(conn: &PgConnection, drop_id: i32) -> Self {
        use crate::schema::drops::dsl::*;
        drops
            .filter(id.eq(drop_id))
            .load::<Self>(conn)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    pub fn into_profile_pic(&self, conn: &PgConnection) -> String {
        let item = Item::fetch(&conn, self.item_id);
        match item.item_type {
            ItemType::ProfilePic { filename } => filename,
            _ => panic!("Item is not a profile picture"),
        }
    }

    pub fn into_background_style(&self, conn: &PgConnection) -> String {
        let item = Item::fetch(&conn, self.item_id);
        match item.item_type {
            ItemType::ProfileBackground { colors } => {
                let mut style = format!(
                    r#"background: linear-gradient({}deg"#,
                    // Convert patten to unsigned integer and then convert to a
                    // degree value.
                    ((self.pattern as u16) as f32 / (u16::MAX as f32) * 360.0) as u16,
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
                                    pattern: rand::random(),
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

#[rocket::get("/item/<drop_id>")]
pub fn item(_user: User, drop_id: i32) -> Template {
    #[derive(Serialize)]
    struct Context {
        id: i32,
        name: String,
        description: String,
        pattern: u16,
        rarity: String,
        thumbnail: String,
    }

    let conn = crate::establish_db_connection().unwrap();

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
            thumbnail: item.thumbnail,
        },
    )
}

/// A trade between two users.
#[derive(Queryable)]
pub struct Trade {
    /// Id of the trade.
    pub id: i32,
    /// UserId of the sender.
    pub sender: i32,
    /// Items offered for trade (expressed as a vec of OwnedItemIds).
    pub sender_items: Vec<i32>,
    /// UserId of the receiver.
    pub receiver: i32,
    /// Items requested for trade
    pub receiver_items: Vec<i32>,
}
