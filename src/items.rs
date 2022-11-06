use std::{cmp::PartialEq, collections::HashMap};

use axum::{
    extract::{Extension, Form, Path},
    Json,
};
use chrono::{Duration, Utc};
use derive_more::From;
use lazy_static::lazy_static;
use maplit::hashmap;
use marche_proc_macros::json_result;
use rand::{prelude::*, Rng, SeedableRng};
use rand_xorshift::XorShiftRng;
use serde::{Deserialize, Serialize};
use sqlx::{
    types::Json as Jsonb, Acquire, Connection, FromRow, PgConnection, PgExecutor, PgPool, Postgres,
    Type,
};

use crate::{
    post,
    users::{ProfileStub, User /*, UserCache*/},
};

/// Rarity of an item.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, Type)]
#[sqlx(type_name = "rarity")]
#[sqlx(rename_all = "snake_case")]
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

/// The type of an item. Determines if the item has any associated actions or is
/// purely cosmetic, and further if the item is cosmetic how many can be
/// equipped
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ItemType {
    /// An item with no use
    Useless,
    /// Cosmetic profile picture, displayable in user profile and next to all
    /// posts
    Avatar { filename: String },
    /// Cosmetic background, displayed behind the profile
    ProfileBackground { colors: Vec<String> },
    /// Reaction image, consumable as an attachment to posts
    Reaction {
        /// Image file for the reaction
        filename: String,
        /// Amount of experience granted to the poster. Value can be negative
        xp_value: i32,
    },
    /// Badge
    Badge { value: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttributeMap {
    map: HashMap<String, AttrInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AttrInfo {
    rarity: u32,
    seed: usize,
}

/// An item that can be dropped
#[derive(FromRow, Debug)]
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
    pub item_type: Jsonb<ItemType>,
    /// Attribute rarity
    pub attributes: Jsonb<AttributeMap>,
}

impl Item {
    pub async fn fetch(
        conn: impl PgExecutor<'_>,
        item_id: i32,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM items WHERE id = $1")
            .bind(item_id)
            .fetch_optional(conn)
            .await
    }

    pub fn is_reaction(&self) -> bool {
        matches!(*self.item_type, ItemType::Reaction { .. })
    }

    pub fn is_equipable(&self) -> bool {
        matches!(
            *self.item_type,
            ItemType::Avatar { .. } | ItemType::ProfileBackground { .. } | ItemType::Badge { .. }
        )
    }
}

/// A dropped item associated with a user
#[derive(FromRow, Debug)]
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

impl PartialEq for ItemDrop {
    fn eq(&self, rhs: &ItemDrop) -> bool {
        self.id == rhs.id
    }
}

lazy_static::lazy_static! {
    /// The minimum amount of time you are aloud to receive a single drop during.
    static ref MIN_DROP_PERIOD: Duration = Duration::minutes(30);
    /// The maximum amount of time since the last drop until the drop is guaranteed.
    static ref MAX_DROP_PERIOD: Duration = Duration::hours(23);
}

/// Chance of drop is equal to 1/DROP_CHANCE
pub const DROP_CHANCE: u32 = 2;

impl ItemDrop {
    pub async fn fetch(
        conn: impl PgExecutor<'_>,
        drop_id: i32,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM drops WHERE id = $1")
            .bind(drop_id)
            .fetch_optional(conn)
            .await
    }

    pub async fn fetch_item(&self, conn: impl PgExecutor<'_>) -> Result<Option<Item>, sqlx::Error> {
        Item::fetch(conn, self.item_id).await
    }

    /// Equips an item. Up to the caller to ensure current user owns the item.
    pub async fn equip(&self, conn: &PgPool) -> Result<(), EquipError> {
        let mut transaction = conn.begin().await?;
        // TODO: Fix unwraps
        let user = User::fetch(&mut transaction, self.owner_id).await?.unwrap();
        let item = Item::fetch(&mut transaction, self.item_id).await?.unwrap();
        let user: User = match *item.item_type {
            ItemType::Avatar { .. } => {
                sqlx::query_as("UPDATE users SET equip_slot_prof_pic = $1 WHERE id = $2")
                    .bind(self.id)
                    .bind(user.id)
                    .fetch_one(&mut transaction)
                    .await?
            }
            ItemType::ProfileBackground { .. } => {
                sqlx::query_as("UPDATE users SET equip_slot_background = $1 WHERE id = $2")
                    .bind(self.id)
                    .bind(user.id)
                    .fetch_one(&mut transaction)
                    .await?
            }
            ItemType::Badge { .. } => {
                let mut badges = user.equip_slot_badges.clone();
                if !badges.contains(&self.id) && badges.len() < crate::users::MAX_NUM_BADGES {
                    badges.push(self.id);
                }
                sqlx::query_as("UPDATE users SET equip_slot_badges = $1 WHERE id = $2")
                    .bind(badges)
                    .bind(user.id)
                    .fetch_one(&mut transaction)
                    .await?
            }
            _ => return Err(EquipError::Unequipable),
        };
        if user.id == self.owner_id {
            transaction.commit().await?;
            Ok(())
        } else {
            Err(EquipError::NotYourItem)
        }
    }

    /// Unequips an item. Up to the caller to ensure current user owns the item.
    pub async fn unequip(
        &self,
        conn: impl Acquire<'_, Database = Postgres>,
    ) -> Result<(), sqlx::Error> {
        let mut conn = conn.acquire().await?;
        let user = User::fetch(&mut *conn, self.owner_id).await?.unwrap();
        let item = Item::fetch(&mut *conn, self.item_id).await?.unwrap();
        match *item.item_type {
            ItemType::Avatar { .. } => {
                sqlx::query("UPDATE users SET equip_slot_prof_pic = $1 WHERE id = $2 && equip_slot_prof_pic = $3")
                    .bind(Option::<i32>::None)
                    .bind(user.id)
                    .bind(self.id)
                    .execute(&mut *conn).await?;
            }
            ItemType::ProfileBackground { .. } => {
                sqlx::query("UPDATE users SET equip_slot_background = $1 WHERE id = $2 && equip_slot_background = $3")
                    .bind(Option::<i32>::None)
                    .bind(user.id)
                    .bind(self.id)
                    .execute(&mut *conn).await?;
            }
            ItemType::Badge { .. } => {
                let badges = user
                    .equip_slot_badges
                    .iter()
                    .cloned()
                    .filter(|drop_id| *drop_id != self.id)
                    .collect::<Vec<_>>();
                sqlx::query("UPDATE users SET equip_slot_badges = $1 WHERE id = $2")
                    .bind(badges)
                    .bind(user.id)
                    .execute(&mut *conn)
                    .await?;
            }
            _ => (),
        }
        Ok(())
    }

    pub async fn is_equiped(&self, conn: &PgPool) -> Result<bool, sqlx::Error> {
        Ok(User::fetch(conn, self.owner_id)
            .await?
            .unwrap()
            .equipped(conn)
            .await?
            .contains(&self))
    }

    /*

            pub fn is_equipped(&self, conn: &PgConnection) -> bool {
                User::fetch(conn, self.owner_id)
                    .unwrap()
                    .equipped(conn)
                    .unwrap()
                    .contains(&self)
            }

            pub fn thumbnail_html(&self, conn: &PgConnection) -> String {
                let item = Item::fetch(&conn, self.item_id);
                let attrs = Attributes::fetch(&item, self);
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
                            self.as_background_style(conn)
                        )
                    }
                    ItemType::Reaction { filename, .. } => format!(
                        r#"
        <div style="animation: start, {div_animation};">
            <img src="/static/{filename}.png"
                 style="width: 50px;
                        height: auto;
                        transform: {transform};
                        animation: start, {animation};
                        filter: {filter};">
        </div>"#,
                        filename = filename,
                        div_animation = attrs.div_animation,
                        transform = attrs.transform,
                        animation = attrs.animation,
                        filter = attrs.filter,
                    ),
                    ItemType::Badge { value } => format!(
                        r#"<div style="font-size: 200%;
                                       text-shadow: 1px 0 white,
                                                    0 1px white,
                                                   -1px 0 white,
                                                    0 -1px white;
                                      ">{value}</div>"#
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
                    description: item.description,
                }
            }

            pub fn as_profile_pic(&self, conn: &PgConnection) -> String {
                let item = Item::fetch(&conn, self.item_id);
                match item.item_type {
                    ItemType::Avatar { filename } => filename,
                    _ => panic!("Item is not a profile picture"),
                }
            }

            pub fn as_background_style(&self, conn: &PgConnection) -> String {
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

            pub fn as_badge(&self, conn: &PgConnection) -> String {
                let item = Item::fetch(&conn, self.item_id);
                match item.item_type {
                    ItemType::Badge { value } => format!("<div>{}</div>", value),
                    _ => panic!("Item is not a badge"),
                }
    }
         */

    /// Possibly selects an item, depending on the last drop.
    pub async fn drop(conn: &PgPool, user: &User) -> Result<Option<Self>, sqlx::Error> {
        // Determine if we have a drop
        if !(user.last_reward < (Utc::now() - *MAX_DROP_PERIOD).naive_utc()
            || user.last_reward < (Utc::now() - *MIN_DROP_PERIOD).naive_utc()
                && rand::random::<u32>() <= (u32::MAX / DROP_CHANCE))
        {
            return Ok(None);
        }

        let chosen: Item =
            sqlx::query_as("SELECT * FROM items WHERE rarity = $1 && available = TRUE")
                .bind(Rarity::roll())
                .fetch_all(conn)
                .await?
                .into_iter()
                .choose(&mut thread_rng())
                .unwrap();

        let mut transaction = conn.begin().await?;

        // Give the new item to the user
        let item_drop = sqlx::query_as(
            "INSERT INTO drops (owner_id, item_id, pattern, consumed) VALUES ($1, $2, $3, FALSE)",
        )
        .bind(user.id)
        .bind(chosen.id)
        .bind(rand::random::<i32>())
        .fetch_one(&mut transaction)
        .await?;

        // Update the last reward. This will fail if the user has seen a reward
        // since the start of this function.
        if user.update_last_reward(&mut transaction).await? {
            // Row was update, commit the transaction
            transaction.commit().await?;
            Ok(Some(item_drop))
        } else {
            transaction.rollback().await?;
            Ok(None)
        }
    }
}

#[derive(Serialize, From)]
pub enum EquipError {
    NoSuchItem,
    NotYourItem,
    Unequipable,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

/*

post! {
    "/equip/:item_id",
    #[json_result]
    async fn equip(
        pool: Extension<PgPool>,
        user: User,
        Path(drop_id): Path<i32>
    ) -> Json<Result<(), EquipError>> {
        let conn = pool.get().expect("Could not connect to db");
        let drop = ItemDrop::fetch(&conn, drop_id).map_err(|_| EquipError::NoSuchItem)?;
        if drop.owner_id == user.id {
            drop.equip(&conn);
        } else {
            return Err(EquipError::NotYourItem);
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub enum UnequipError {
    NoSuchItem,
    NotYourItem,
}

post! {
    "/unequip/:item_id",
    #[json_result]
    pub async fn unequip(
        pool: Extension<PgPool>,
        user: User,
        Path(drop_id): Path<i32>
    ) -> Json<Result<(), UnequipError>> {
        let conn = pool.get().expect("Could not connect to db");
        let drop = ItemDrop::fetch(&conn, drop_id).map_err(|_|UnequipError::NoSuchItem)?;
        if drop.owner_id == user.id {
            drop.unequip(&conn);
        } else {
            return Err(UnequipError::NotYourItem);
        }

        Ok(())
    }
}

// TODO: Take this struct and extract it somewhere
#[derive(Serialize)]
pub struct ItemThumbnail {
    pub id: i32,
    pub name: String,
    pub rarity: String,
    pub thumbnail: String,
    pub description: String,
}

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub enum AttributeType {
    Filter,
    DivAnimation,
    Animation,
    Transform,
}

pub struct Attribute {
    pub ty: AttributeType,
    pub fmt_fn: fn(&mut XorShiftRng) -> String,
}

impl Attribute {
    fn fmt(&self, rng: &mut XorShiftRng) -> String {
        (self.fmt_fn)(rng)
    }

    fn filter(fmt_fn: fn(&mut XorShiftRng) -> String) -> Self {
        Self {
            ty: AttributeType::Filter,
            fmt_fn,
        }
    }

    fn div_animation(fmt_fn: fn(&mut XorShiftRng) -> String) -> Self {
        Self {
            ty: AttributeType::DivAnimation,
            fmt_fn,
        }
    }

    fn animation(fmt_fn: fn(&mut XorShiftRng) -> String) -> Self {
        Self {
            ty: AttributeType::Animation,
            fmt_fn,
        }
    }

    fn transform(fmt_fn: fn(&mut XorShiftRng) -> String) -> Self {
        Self {
            ty: AttributeType::Transform,
            fmt_fn,
        }
    }
}

lazy_static! {
    pub static ref ATTRIBUTES: HashMap<&'static str, Attribute> = {
        hashmap! {
            // Animations:
            "spin" => Attribute::animation(|rng| format!("spin {}s infinite linear", rng.gen_range::<f32, _>(0.1..3.0))),
            "shiny" => Attribute::animation(|rng| format!("shiny {}s infinite linear", rng.gen_range::<f32, _>(0.33..3.0))),

            // Div Animations:
            "rolling" => Attribute::div_animation(|rng| format!("roll {}s infinite linear", rng.gen_range::<f32, _>(0.33..3.0))),

            // Transforms:
            "rotation" => Attribute::transform(|rng| format!("rotate({}deg)", rng.gen_range::<f32, _>(0.0..360.0))),

            // Filters:
            "blur" => Attribute::filter(|rng| format!("blur({}px)", rng.gen_range::<u16, _>(2..8))),
            "transparency" => Attribute::filter(|rng| format!("opacity({}%)", rng.gen_range::<f32,_>(10.0..60.0))),
            "contrast" => Attribute::filter(|rng| format!("contrast({}%)", rng.gen_range::<f32,_>(100.0..500.0))),
            "sepia" => Attribute::filter(|_| format!("sepia(100%)")),
            "inverted" => Attribute::filter(|_| format!("invert(100%)")),
            "saturation" => Attribute::filter(|rng| format!("saturate({}%)", rng.gen_range::<f32, _>(100.0..400.0))),
        }
    };
}

#[derive(Debug)]
pub struct Attributes {
    pub filter: String,
    pub div_animation: String,
    pub animation: String,
    pub transform: String,
}

impl Attributes {
    fn fetch(item: &Item, item_drop: &ItemDrop) -> Self {
        let attributes = item.attributes.clone().map;
        let mut attr_res = HashMap::new();

        for (attr_name, AttrInfo { rarity, seed }) in attributes.into_iter() {
            let mut rng =
                XorShiftRng::seed_from_u64((item_drop.pattern as u64) << 32 | seed as u64); // TODO: make pattern a u64?

            if rng.gen_ratio(1, rarity) {
                let attr = ATTRIBUTES.get(&*attr_name).unwrap();
                attr_res
                    .entry(attr.ty)
                    .or_insert_with(Vec::new)
                    .push(attr.fmt(&mut rng));
            }
        }

        Self {
            filter: attr_res
                .remove(&AttributeType::Filter)
                .as_deref()
                .unwrap_or(&[])
                .join(" "),
            div_animation: attr_res
                .remove(&AttributeType::DivAnimation)
                .as_deref()
                .unwrap_or(&[])
                .join(", "),
            animation: attr_res
                .remove(&AttributeType::Animation)
                .as_deref()
                .unwrap_or(&[])
                .join(", "),
            transform: attr_res
                .remove(&AttributeType::Transform)
                .as_deref()
                .unwrap_or(&[])
                .join(" "),
        }
    }
}
*/

/// A trade between two users.
#[derive(Serialize, FromRow)]
pub struct TradeRequest {
    /// Id of the trade
    pub id: i32,
    /// UserId of the sender
    pub sender_id: i32,
    /// Items offered for trade (expressed as a vec of OwnedItemIds)
    pub sender_items: Vec<i32>,
    /// UserId of the receiver
    pub receiver_id: i32,
    /// Items requested for trade
    pub receiver_items: Vec<i32>,
    /// Any note attached to this request
    pub note: Option<String>,
}

#[derive(Serialize, From)]
pub enum TradeResponseError {
    NoSuchTrade,
    NotYourTrade,
    ConflictingTradeExecuted,
    InternalDbError(#[serde(skip)] sqlx::Error),
}

impl TradeRequest {
    pub async fn fetch(conn: &PgPool, id: i32) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM trade_requests WHERE id = $1")
            .bind(id)
            .fetch_optional(conn)
            .await
    }

    pub async fn accept(&self, conn: &PgPool) -> Result<(), TradeResponseError> {
        let mut transaction = conn.begin().await?;

        for sender_item in &self.sender_items {
            ItemDrop::fetch(&mut transaction, *sender_item)
                .await?
                .unwrap()
                .unequip(&mut transaction)
                .await?;
            sqlx::query("UPDATE drops SET owner_id = $1 WHERE id = $2 && owner_id = $3")
                .bind(self.receiver_id)
                .bind(*sender_item)
                .bind(self.sender_id)
                .execute(&mut transaction)
                .await?;
        }

        for receiver_item in &self.receiver_items {
            ItemDrop::fetch(&mut transaction, *receiver_item)
                .await?
                .unwrap()
                .unequip(&mut transaction)
                .await?;

            sqlx::query("UPDATE drops SET owner_id = $1 WHERE id = $2 && owner_id = $3")
                .bind(self.sender_id)
                .bind(*receiver_item)
                .bind(self.receiver_id)
                .execute(&mut transaction)
                .await?;
        }

        // Check if any item no longer belongs to their respective owner.
        // Consumed items may not be traded.

        for sender_item in &self.sender_items {
            let item_drop: ItemDrop =
                sqlx::query_as("SELECT * FROM drops WHERE id = $1 && consumed = FALSE")
                    .bind(*sender_item)
                    .fetch_one(&mut transaction)
                    .await?;
            if item_drop.owner_id != self.receiver_id {
                transaction.rollback().await?;
                return Err(TradeResponseError::ConflictingTradeExecuted);
            }
        }

        for receiver_item in &self.receiver_items {
            let item_drop: ItemDrop =
                sqlx::query_as("SELECT * FROM drops WHERE id = $1 && consumed = FALSE")
                    .bind(*receiver_item)
                    .fetch_one(&mut transaction)
                    .await?;
            if item_drop.owner_id != self.sender_id {
                transaction.rollback().await?;
                return Err(TradeResponseError::ConflictingTradeExecuted);
            }
        }

        // Delete the transaction
        self.decline(&mut *transaction).await?;

        Ok(())
    }

    pub async fn decline(&self, conn: impl PgExecutor<'_>) -> Result<(), TradeResponseError> {
        /*
                use crate::schema::trade_requests::dsl::*;

                diesel::delete(trade_requests.find(self.id)).execute(conn)?;
        */
        Ok(())
    }
}

/*
#[derive(Debug, Deserialize)]
pub struct TradeRequestForm {
    receiver_id: String,
    note: Option<String>,
    #[serde(flatten)]
    trade: HashMap<String, String>,
}

#[derive(Serialize, From)]
pub enum SubmitOfferError {
    CannotTradeWithSelf,
    ItemNoLongerOwned,
    NoSuchUser,
    InvalidTrade,
    NoteTooLong,
    TradeIsEmpty,
    InternalDbError(#[serde(skip)] sqlx::Error),
    InvalidForm(#[serde(skip)] std::num::ParseIntError),
}

const MAX_NOTE_LENGTH: usize = 150;

post! {
    "/offer/",
    #[json_result]
    pub async fn submit_offer(
        pool: Extension<PgPool>,
        sender: User,
        Form(TradeRequestForm { receiver_id, note, trade }): Form<TradeRequestForm>,
    ) -> Json<Result<TradeRequest, SubmitOfferError>> {
        let mut sender_items = Vec::new();
        let mut receiver_items = Vec::new();

        let receiver_id: i32 = receiver_id.parse().map_err(|_|SubmitOfferError::NoSuchUser)?;

        if sender.id == receiver_id {
            return Err(SubmitOfferError::CannotTradeWithSelf);
        }

        let conn = &mut pool.get().expect("Could not connect to db");

        if User::fetch(&conn, receiver_id).is_err() {
            return Err(SubmitOfferError::NoSuchUser);
        }

        for (item, trader) in trade.into_iter() {
            let item: i32 = item.parse()?;
            let trader: i32 = trader.parse()?;

            let drop = ItemDrop::fetch(&conn, item)?;
            if trader != drop.owner_id {
                return Err(SubmitOfferError::ItemNoLongerOwned);
            }
            if trader == sender.id {
                sender_items.push(drop.id);
            } else if trader == receiver_id {
                receiver_items.push(drop.id);
            } else {
                return Err(SubmitOfferError::InvalidTrade);
            }
        }

        if sender_items.is_empty() && receiver_items.is_empty() {
            return Err(SubmitOfferError::TradeIsEmpty);
        }

        let note = note
            .and_then(|note| {
                let trimmed = note.trim();
                (!trimmed.is_empty()).then(|| {
                    if trimmed.len() > MAX_NOTE_LENGTH {
                        Err(SubmitOfferError::NoteTooLong)
                    } else {
                        Ok(trimmed.to_string())
                    }
                })
            })
            .transpose()?;

        Ok(diesel::insert_into(crate::schema::trade_requests::table)
            .values(&NewTradeRequest {
                sender_id: sender.id,
                sender_items,
                receiver_id,
                receiver_items,
                note,
            })
           .get_result::<TradeRequest>(conn)?)
    }
}

post! {
    "/accept/:trade_id",
    #[json_result]
    async fn accept(
        pool: Extension<PgPool>,
        user: User,
        Path(trade_id): Path<i32>
    ) -> Json<Result<(), TradeResponseError>> {
        let conn = &mut pool.get().expect("Could not connect to db");
        let req = TradeRequest::fetch(conn, trade_id)?;
        if req.receiver_id == user.id {
            req.accept(conn)
        } else {
            Err(TradeResponseError::NotYourTrade)
        }
    }
}

post! {
    "/decline/:trade_id",
    #[json_result]
    async fn decline_offer(
        pool: Extension<PgPool>,
        user: User,
        Path(trade_id): Path<i32>,
    ) -> Json<Result<(), TradeResponseError>> {
        let conn = &mut pool.get().expect("Could not connect to db");
        let req = TradeRequest::fetch(conn, trade_id)?;
        if req.sender_id == user.id || req.receiver_id == user.id {
            req.decline(conn)
        } else {
            Err(TradeResponseError::NotYourTrade)
        }
    }
}

/*

#[derive(Serialize)]
pub struct IncomingOffer {
    pub id: i32,
    pub sender: ProfileStub,
    pub sender_items: Vec<ItemThumbnail>,
    pub receiver_items: Vec<ItemThumbnail>,
    pub note: Option<String>,
}

impl IncomingOffer {
    pub fn retrieve(
        conn: &PgConnection,
        user_cache: &mut UserCache,
        user: &User,
    ) -> Vec<IncomingOffer> {
        use crate::schema::trade_requests::dsl::*;

        return trade_requests
            .filter(receiver_id.eq(user.id))
            .load::<TradeRequest>(conn)
            .unwrap()
            .into_iter()
            .map(|trade| -> IncomingOffer {
                IncomingOffer {
                    id: trade.id,
                    sender: user_cache.get(trade.sender_id).clone(),
                    sender_items: trade
                        .sender_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    receiver_items: trade
                        .receiver_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    note: trade.note,
                }
            })
            .collect();
    }

    pub fn count(conn: &PgConnection, user: &User) -> i64 {
        use crate::schema::trade_requests::dsl::*;
        return trade_requests
            .filter(receiver_id.eq(user.id))
            .count()
            .get_result(conn)
            .unwrap_or(0);
    }
}

#[derive(Serialize)]
pub struct OutgoingOffer {
    pub id: i32,
    pub sender_items: Vec<ItemThumbnail>,
    pub receiver: ProfileStub,
    pub receiver_items: Vec<ItemThumbnail>,
    pub note: Option<String>,
}

impl OutgoingOffer {
    pub fn retrieve(
        conn: &PgConnection,
        user_cache: &mut UserCache,
        user: &User,
    ) -> Vec<OutgoingOffer> {
        use crate::schema::trade_requests::dsl::*;

        return trade_requests
            .filter(sender_id.eq(user.id))
            .load::<TradeRequest>(conn)
            .unwrap()
            .into_iter()
            .map(|trade| -> OutgoingOffer {
                OutgoingOffer {
                    id: trade.id,
                    receiver: user_cache.get(trade.receiver_id).clone(),
                    sender_items: trade
                        .sender_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    receiver_items: trade
                        .receiver_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    note: trade.note,
                }
            })
            .collect();
    }
}
*/
*/
