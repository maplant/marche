use std::{cmp::PartialEq, collections::HashMap, num::ParseIntError, str::FromStr, sync::Arc};

use axum::extract::{Extension, Form, Path, Query};
use chrono::{Duration, Utc};
use futures::{future, StreamExt};
use lazy_static::lazy_static;
use maplit::hashmap;
use marche_proc_macros::{json, ErrorCode};
use rand::{prelude::*, Rng, SeedableRng};
use rand_xorshift::XorShiftRng;
use serde::{Deserialize, Serialize};
use sqlx::{
    types::Json as Jsonb, Acquire, FromRow, PgExecutor, PgPool, Postgres, Transaction, Type,
};
use thiserror::Error;

use crate::{
    images::{Image, UploadImageError, MAXIMUM_FILE_SIZE},
    post,
    users::{ProfileStub, Role, User, UserCache},
    MultipartForm, MultipartFormError,
};

/// Rarity of an item.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord, Type, Serialize)]
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

#[derive(Copy, Clone, Debug, Error)]
#[error("invalid rarity")]
pub struct InvalidRarity;

impl FromStr for Rarity {
    type Err = InvalidRarity;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "common" => Ok(Self::Common),
            "uncommon" => Ok(Self::Uncommon),
            "rare" => Ok(Self::Rare),
            "ultra-rare" => Ok(Self::UltraRare),
            "legendary" => Ok(Self::Legendary),
            "unique" => Ok(Self::Unique),
            _ => Err(InvalidRarity),
        }
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
    #[serde(flatten)]
    pub attrs: HashMap<String, AttrInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttrInfo {
    rarity: u32,
    seed:   usize,
}

/// An item that can be dropped
#[derive(FromRow, Debug, Serialize, Clone)]
pub struct Item {
    /// Id of the available item
    pub id:          i32,
    /// Name of the item
    pub name:        String,
    /// Description of the item
    pub description: String,
    /// Availability of the item (can the item be dropped?)
    pub available:   bool,
    /// Rarity of the item
    pub rarity:      Rarity,
    /// Type of the item
    pub item_type:   Jsonb<ItemType>,
    /// Attribute rarity
    pub attributes:  Jsonb<AttributeMap>,
}

impl Item {
    pub async fn fetch(conn: impl PgExecutor<'_>, item_id: i32) -> Result<Self, sqlx::Error> {
        sqlx::query_as("SELECT * FROM items WHERE id = $1")
            .bind(item_id)
            .fetch_one(conn)
            .await
    }

    pub async fn fetch_optional(
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

    pub fn as_avatar(&self) -> Option<String> {
        match self.item_type {
            Jsonb(ItemType::Avatar { ref filename }) => Some(filename.clone()),
            _ => None,
        }
    }

    pub fn as_profile_background(&self, pattern: i32) -> Option<String> {
        match self.item_type {
            Jsonb(ItemType::ProfileBackground { ref colors }) => {
                let mut style = format!(
                    r#"background: linear-gradient({}deg"#,
                    // Convert patten to unsigned integer and then convert to a
                    // degree value.
                    (pattern as u32) as f32 / (u16::MAX as f32) * 360.0,
                );
                for color in colors {
                    style += ", ";
                    style += &color;
                }
                style += ");";
                Some(style)
            }
            _ => None,
        }
    }

    pub fn as_badge(&self) -> Option<String> {
        match self.item_type {
            Jsonb(ItemType::Badge { ref value }) => Some(format!("<div>{value}</div>")),
            _ => None,
        }
    }

    pub fn get_experience(&self) -> Option<i32> {
        match self.item_type {
            Jsonb(ItemType::Reaction { xp_value, .. }) => Some(xp_value),
            _ => None,
        }
    }

    pub fn get_thumbnail_html(&self, pattern: i32) -> String {
        let Attributes {
            div_animation,
            transform,
            filter,
            animation,
        } = Attributes::fetch(self, pattern);
        match self.item_type.0 {
            ItemType::Useless => String::from(r#"<div class="fixed-item-thumbnail">?</div>"#),
            ItemType::Avatar { ref filename } => {
                format!(r#"<img src="{filename}" style="width: 50px; height: auto;">"#,)
            }
            ItemType::ProfileBackground { .. } => {
                format!(
                    r#"<div class="fixed-item-thumbnail" style="{}"></div>"#,
                    self.as_profile_background(pattern).unwrap()
                )
            }
            ItemType::Reaction { ref filename, .. } => format!(
                r#"
                <div style="animation: start, {div_animation};">
                    <img src="{filename}"
                         style="width: 50px;
                                height: auto;
                                transform: {transform};
                                animation: start, {animation};
                                 filter: {filter};">
                </div>
                "#
            ),
            ItemType::Badge { ref value } => format!(
                r#"
                <div style="font-size: 200%;
                            text-shadow: 1px 0 white,
                                         0 1px white,
                                         -1px 0 white,
                                         0 -1px white;">
                    {value}
                </div>
                "#
            ),
        }
    }
}

#[derive(Deserialize)]
struct SetAvailability {
    available: bool,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
enum SetAvailabilityError {
    #[error("You are not authorized to make items available to drop")]
    Unauthorized,
    #[error("Internal db error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/set_item_availability/:item_id",
    #[json]
    async fn set_availability(
        conn: Extension<PgPool>,
        user: User,
        Path(item_id): Path<i32>,
        Query(SetAvailability { available }): Query<SetAvailability>,
    ) -> Result<(), SetAvailabilityError> {
        if user.role < Role::Admin {
            return Err(SetAvailabilityError::Unauthorized);
        }

        sqlx::query("UPDATE items SET available = $1 WHERE id = $2")
            .bind(available)
            .bind(item_id)
            .execute(&*conn)
            .await?;

        Ok(())
    }
);

/// A dropped item associated with a user
#[derive(FromRow, Debug, Clone)]
pub struct ItemDrop {
    /// Id of the dropped item
    pub id:       i32,
    /// UserId of the owner
    pub owner_id: i32,
    /// ItemId of the item
    pub item_id:  i32,
    /// Unique pattern Id for the item
    pub pattern:  i32,
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
    pub async fn fetch_optional(
        conn: impl PgExecutor<'_>,
        drop_id: i32,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM drops WHERE id = $1")
            .bind(drop_id)
            .fetch_optional(conn)
            .await
    }

    pub async fn fetch(conn: impl PgExecutor<'_>, drop_id: i32) -> Result<Self, sqlx::Error> {
        sqlx::query_as("SELECT * FROM drops WHERE id = $1")
            .bind(drop_id)
            .fetch_one(conn)
            .await
    }

    pub async fn fetch_item(&self, conn: impl PgExecutor<'_>) -> Result<Item, sqlx::Error> {
        Item::fetch(conn, self.item_id).await
    }

    pub fn to_id(self) -> i32 {
        self.id
    }

    /// Equips an item. Up to the caller to ensure current user owns the item.
    pub async fn equip(&self, conn: &PgPool) -> Result<(), EquipError> {
        let mut transaction = conn.begin().await?;
        // TODO: Fix unwraps
        let user = User::fetch(&mut transaction, self.owner_id).await?;
        let item = Item::fetch(&mut transaction, self.item_id).await?;
        let user: User = match *item.item_type {
            ItemType::Avatar { .. } => {
                sqlx::query_as(
                    "UPDATE users SET equip_slot_prof_pic = $1 WHERE id = $2 RETURNING *",
                )
                .bind(self.id)
                .bind(user.id)
                .fetch_one(&mut transaction)
                .await?
            }
            ItemType::ProfileBackground { .. } => {
                sqlx::query_as(
                    "UPDATE users SET equip_slot_background = $1 WHERE id = $2 RETURNING *",
                )
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
                sqlx::query_as("UPDATE users SET equip_slot_badges = $1 WHERE id = $2 RETURNING *")
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
            Err(EquipError::Unauthorized)
        }
    }

    /// Unequips an item. Up to the caller to ensure current user owns the item.
    pub fn unequip<'a, 'c>(
        &'a self,
        conn: impl Acquire<'c, Database = Postgres> + Send + 'a,
    ) -> impl std::future::Future<Output = Result<(), sqlx::Error>> + Send + 'a {
        async move {
            let mut conn = conn.acquire().await?;
            let user = User::fetch(&mut *conn, self.owner_id).await?;
            let item = Item::fetch(&mut *conn, self.item_id).await?;
            match *item.item_type {
                ItemType::Avatar { .. } => {
                    sqlx::query("UPDATE users SET equip_slot_prof_pic = $1 WHERE id = $2 AND equip_slot_prof_pic = $3")
                    .bind(Option::<i32>::None)
                    .bind(user.id)
                    .bind(self.id)
                    .execute(&mut *conn).await?;
                }
                ItemType::ProfileBackground { .. } => {
                    sqlx::query("UPDATE users SET equip_slot_background = $1 WHERE id = $2 AND equip_slot_background = $3")
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
    }

    /// Possibly selects an item, depending on the last drop.
    pub async fn drop(
        conn: &mut Transaction<'_, Postgres>,
        user: &User,
    ) -> Result<Option<Self>, sqlx::Error> {
        // Determine if we have a drop
        if !(user.last_reward < (Utc::now() - *MAX_DROP_PERIOD).naive_utc()
            || user.last_reward < (Utc::now() - *MIN_DROP_PERIOD).naive_utc()
                && rand::random::<u32>() <= (u32::MAX / DROP_CHANCE))
        {
            return Ok(None);
        }

        let conn = conn.acquire().await?;

        let chosen: Option<Item> =
            sqlx::query_as("SELECT * FROM items WHERE rarity = $1 AND available = TRUE")
                .bind(Rarity::roll())
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .choose(&mut thread_rng());
        let Some(chosen) = chosen else { return Ok(None); };

        let mut transaction = (&mut *conn).begin().await?;

        // Give the new item to the user
        let item_drop = sqlx::query_as(
            r#"
            INSERT INTO drops (owner_id, item_id, pattern, consumed)
            VALUES ($1, $2, $3, FALSE)
            RETURNING *
            "#,
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

    pub async fn get_thumbnail(&self, conn: &PgPool) -> Result<ItemThumbnail, sqlx::Error> {
        let item = self.fetch_item(conn).await?;
        Ok(ItemThumbnail::new(&item, self))
    }
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum EquipError {
    #[error("No such item exists")]
    NoSuchItem,
    #[error("That is not your item")]
    Unauthorized,
    #[error("That item cannot be equiped")]
    Unequipable,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post! {
    "/equip/:item_id",
    #[json]
    async fn equip(
        conn: Extension<PgPool>,
        user: User,
        Path(drop_id): Path<i32>
    ) -> Result<(), EquipError> {
        let drop = ItemDrop::fetch_optional(&*conn, drop_id).await?.ok_or(EquipError::NoSuchItem)?;
        if drop.owner_id == user.id {
            drop.equip(&*conn).await?;
        } else {
            return Err(EquipError::Unauthorized);
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum UnequipError {
    #[error("No such item exists")]
    NoSuchItem,
    #[error("That is not your item")]
    Unauthorized,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post! {
    "/unequip/:item_id",
    #[json]
    pub async fn unequip(
        conn: Extension<PgPool>,
        user: User,
        Path(drop_id): Path<i32>
    ) -> Result<(), UnequipError> {
        let drop = ItemDrop::fetch_optional(&*conn, drop_id).await?.ok_or(UnequipError::NoSuchItem)?;
        if drop.owner_id == user.id {
            drop.unequip(&*conn).await?;
        } else {
            return Err(UnequipError::Unauthorized);
        }

        Ok(())
    }
}

// TODO: Take this struct and extract it somewhere
#[derive(Serialize)]
pub struct ItemThumbnail {
    pub id:          i32,
    pub name:        String,
    pub rarity:      String,
    pub html:        String,
    pub description: String,
}

impl ItemThumbnail {
    pub fn new(item: &Item, item_drop: &ItemDrop) -> Self {
        ItemThumbnail {
            id:          item_drop.id,
            name:        item.name.clone(),
            rarity:      item.rarity.to_string(),
            html:        item.get_thumbnail_html(item_drop.pattern),
            description: item.description.clone(),
        }
    }
}

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub enum AttributeType {
    Filter,
    DivAnimation,
    Animation,
    Transform,
}

pub struct Attribute {
    pub ty:     AttributeType,
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
    pub filter:        String,
    pub div_animation: String,
    pub animation:     String,
    pub transform:     String,
}

impl Attributes {
    fn fetch(item: &Item, pattern: i32) -> Self {
        let attributes = item.attributes.0.clone().attrs;
        let mut attr_res = HashMap::new();

        for (attr_name, AttrInfo { rarity, seed }) in attributes.into_iter() {
            let mut rng = XorShiftRng::seed_from_u64((pattern as u64) << 32 | seed as u64);

            if rng.gen_ratio(1, rarity) {
                let attr = ATTRIBUTES.get(&*attr_name).unwrap();
                attr_res
                    .entry(attr.ty)
                    .or_insert_with(Vec::new)
                    .push(attr.fmt(&mut rng));
            }
        }

        Self {
            filter:        attr_res
                .remove(&AttributeType::Filter)
                .as_deref()
                .unwrap_or(&[])
                .join(" "),
            div_animation: attr_res
                .remove(&AttributeType::DivAnimation)
                .as_deref()
                .unwrap_or(&[])
                .join(", "),
            animation:     attr_res
                .remove(&AttributeType::Animation)
                .as_deref()
                .unwrap_or(&[])
                .join(", "),
            transform:     attr_res
                .remove(&AttributeType::Transform)
                .as_deref()
                .unwrap_or(&[])
                .join(" "),
        }
    }
}

/// A trade between two users.
#[derive(Debug, Serialize, FromRow)]
pub struct TradeRequest {
    /// Id of the trade
    pub id:             i32,
    /// UserId of the sender
    pub sender_id:      i32,
    /// Items offered for trade (expressed as a vec of OwnedItemIds)
    pub sender_items:   Vec<i32>,
    /// UserId of the receiver
    pub receiver_id:    i32,
    /// Items requested for trade
    pub receiver_items: Vec<i32>,
    /// Any note attached to this request
    pub note:           Option<String>,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum TradeResponseError {
    #[error("No such item exists")]
    NoSuchItem,
    #[error("No such trade exists")]
    NoSuchTrade,
    #[error("Not your trade")]
    Unauthorized,
    #[error("A conflicting trade has already been executed")]
    ConflictingTradeExecuted,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
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
            ItemDrop::fetch_optional(&mut transaction, *sender_item)
                .await?
                .ok_or(TradeResponseError::NoSuchItem)?
                .unequip(&mut transaction)
                .await?;

            sqlx::query("UPDATE drops SET owner_id = $1 WHERE id = $2 AND owner_id = $3")
                .bind(self.receiver_id)
                .bind(*sender_item)
                .bind(self.sender_id)
                .execute(&mut transaction)
                .await?;
        }

        for receiver_item in &self.receiver_items {
            ItemDrop::fetch_optional(&mut *transaction, *receiver_item)
                .await?
                .ok_or(TradeResponseError::NoSuchItem)?
                .unequip(&mut transaction)
                .await?;

            sqlx::query("UPDATE drops SET owner_id = $1 WHERE id = $2 AND owner_id = $3")
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
                sqlx::query_as("SELECT * FROM drops WHERE id = $1 AND consumed = FALSE")
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
                sqlx::query_as("SELECT * FROM drops WHERE id = $1 AND consumed = FALSE")
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
        sqlx::query("DELETE FROM trade_requests WHERE id = $1")
            .bind(self.id)
            .execute(conn)
            .await?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct TradeRequestForm {
    receiver_id: String,
    note:        Option<String>,
    #[serde(flatten)]
    trade:       HashMap<String, String>,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum SubmitOfferError {
    #[error("You cannot trade with yourself")]
    CannotTradeWithSelf,
    #[error("Item is no longer owned by one of the parties")]
    ItemNoLongerOwned,
    #[error("No such user")]
    NoSuchUser,
    #[error("Invalid trade")]
    InvalidTrade,
    #[error("Note is too long")]
    NoteTooLong,
    #[error("Trade is empty")]
    TradeIsEmpty,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
    #[error("Invalid form: {0}")]
    InvalidForm(
        #[from]
        #[serde(skip)]
        std::num::ParseIntError,
    ),
}

const MAX_NOTE_LENGTH: usize = 150;

post! {
    "/offer",
    #[json]
    pub async fn submit_offer(
        conn: Extension<PgPool>,
        sender: User,
        Form(TradeRequestForm { receiver_id, note, trade }): Form<TradeRequestForm>,
    ) -> Result<TradeRequest, SubmitOfferError> {
        let mut sender_items = Vec::new();
        let mut receiver_items = Vec::new();

        let receiver_id: i32 = receiver_id.parse()?;

        if sender.id == receiver_id {
            return Err(SubmitOfferError::CannotTradeWithSelf);
        }

        if User::fetch_optional(&*conn, receiver_id).await?.is_none() {
            return Err(SubmitOfferError::NoSuchUser);
        }

        for (item, trader) in trade.into_iter() {
            let item: i32 = item.parse()?;
            let trader: i32 = trader.parse()?;

            let Some(drop) = ItemDrop::fetch_optional(&*conn, item).await? else {
                return Err(SubmitOfferError::InvalidTrade);
            };

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

        Ok(
            sqlx::query_as(
                r#"
                INSERT INTO trade_requests
                    (sender_id, sender_items, receiver_id, receiver_items, note)
                VALUES
                    ($1, $2, $3, $4, $5)
                RETURNING *
                "#
            )
                .bind(sender.id)
                .bind(sender_items)
                .bind(receiver_id)
                .bind(receiver_items)
                .bind(note)
                .fetch_one(&*conn)
                .await?
        )
    }
}

post! {
    "/accept/:trade_id",
    #[json]
    async fn accept(
        conn: Extension<PgPool>,
        user: User,
        Path(trade_id): Path<i32>
    ) -> Result<(), TradeResponseError> {
        let req = TradeRequest::fetch(&*conn, trade_id)
            .await?
            .ok_or(TradeResponseError::NoSuchTrade)?;
        if req.receiver_id == user.id {
            req.accept(&*conn).await
        } else {
            Err(TradeResponseError::Unauthorized)
        }
    }
}

post! {
    "/decline/:trade_id",
    #[json]
    async fn decline_offer(
        conn: Extension<PgPool>,
        user: User,
        Path(trade_id): Path<i32>,
    ) -> Result<(), TradeResponseError> {
        let req = TradeRequest::fetch(&*conn, trade_id)
            .await?
            .ok_or(TradeResponseError::NoSuchTrade)?;
        if req.sender_id == user.id || req.receiver_id == user.id {
            req.decline(&*conn).await
        } else {
            Err(TradeResponseError::Unauthorized)
        }
    }
}

#[derive(Serialize)]
pub struct IncomingOffer {
    pub id:             i32,
    pub sender:         Arc<ProfileStub>,
    pub sender_items:   Vec<ItemThumbnail>,
    pub receiver_items: Vec<ItemThumbnail>,
    pub note:           Option<String>,
}

impl IncomingOffer {
    pub async fn retrieve(
        conn: &PgPool,
        user_cache: &UserCache<'_>,
        user: &User,
    ) -> Vec<IncomingOffer> {
        sqlx::query_as("SELECT * FROM trade_requests WHERE receiver_id = $1")
            .bind(user.id)
            .fetch(conn)
            .filter_map(|trade: Result<TradeRequest, _>| future::ready(trade.ok()))
            .then(|trade| async move {
                let mut sender_items = Vec::new();
                for sender_item in trade.sender_items.into_iter() {
                    sender_items.push(
                        ItemDrop::fetch(conn, sender_item)
                            .await?
                            .get_thumbnail(conn)
                            .await?,
                    );
                }
                let mut receiver_items = Vec::new();
                for receiver_item in trade.receiver_items.into_iter() {
                    receiver_items.push(
                        ItemDrop::fetch(conn, receiver_item)
                            .await?
                            .get_thumbnail(conn)
                            .await?,
                    );
                }
                sqlx::Result::Ok(IncomingOffer {
                    id: trade.id,
                    sender: user_cache.get(trade.sender_id).await?,
                    note: trade.note,
                    sender_items,
                    receiver_items,
                })
            })
            .filter_map(|t| future::ready(t.ok()))
            .collect()
            .await
    }
}

#[derive(Serialize)]
pub struct OutgoingOffer {
    pub id:             i32,
    pub sender_items:   Vec<ItemThumbnail>,
    pub receiver:       Arc<ProfileStub>,
    pub receiver_items: Vec<ItemThumbnail>,
    pub note:           Option<String>,
}

impl OutgoingOffer {
    pub async fn retrieve(
        conn: &PgPool,
        user_cache: &UserCache<'_>,
        user: &User,
    ) -> Vec<OutgoingOffer> {
        sqlx::query_as("SELECT * FROM trade_requests WHERE sender_id = $1")
            .bind(user.id)
            .fetch(conn)
            .filter_map(|trade: Result<TradeRequest, _>| future::ready(trade.ok()))
            .then(|trade| async move {
                let mut sender_items = Vec::new();
                for sender_item in trade.sender_items.into_iter() {
                    sender_items.push(
                        ItemDrop::fetch(conn, sender_item)
                            .await?
                            .get_thumbnail(conn)
                            .await?,
                    );
                }
                let mut receiver_items = Vec::new();
                for receiver_item in trade.receiver_items.into_iter() {
                    receiver_items.push(
                        ItemDrop::fetch(conn, receiver_item)
                            .await?
                            .get_thumbnail(conn)
                            .await?,
                    );
                }
                sqlx::Result::Ok(Self {
                    id: trade.id,
                    receiver: user_cache.get(trade.receiver_id).await?,
                    note: trade.note,
                    sender_items,
                    receiver_items,
                })
            })
            .filter_map(|t| future::ready(t.ok()))
            .collect()
            .await
    }
}

#[derive(Debug, Deserialize)]
pub struct MintItemForm {
    name:       String,
    descr:      String,
    rarity:     String,
    item_type:  String,
    badge:      String,
    experience: String,
    colors:     String,
    attrs:      String,
}

#[derive(Debug, Serialize, Error, ErrorCode)]
pub enum MintItemError {
    #[error("Item name cannot be empty")]
    EmptyName,
    #[error("Item description cannot be empty")]
    EmptyDescription,
    #[error("Invalid rarity")]
    InvalidRarity(
        #[from]
        #[serde(skip)]
        InvalidRarity,
    ),
    #[error("Invalid item type")]
    InvalidItemType,
    #[error("Invalid badge")]
    InvalidBadge,
    #[error("Attributes is not valid json")]
    InvalidAttributes,
    #[error("Background colors is not valid json")]
    InvalidColors(
        #[from]
        #[serde(skip)]
        serde_json::Error,
    ),
    #[error("Experience value is not a valid 32 bit signed integer")]
    InvalidExperience(
        #[from]
        #[serde(skip)]
        ParseIntError,
    ),
    #[error("A file must be attached to create a new reaction or avatar")]
    NoImageAttached,
    #[error("Error uploading image: {0}")]
    UploadImageError(#[from] UploadImageError),
    #[error("No such attribute '{0}' exists")]
    NoSuchAttribute(String),
    #[error("You are not authorized to mint items")]
    Unauthorized,
    #[error("Internal db error {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
    #[error("Multipart form error {0}")]
    MultipartFormError(#[from] MultipartFormError),
}

post!(
    "/mint",
    #[json]
    async fn mint_item(
        conn: Extension<PgPool>,
        user: User,
        form: Result<MultipartForm<MintItemForm, MAXIMUM_FILE_SIZE>, MultipartFormError>,
    ) -> Result<Item, MintItemError> {
        if user.role < Role::Admin {
            return Err(MintItemError::Unauthorized);
        }

        let MultipartForm {
            form:
                MintItemForm {
                    name,
                    descr,
                    rarity,
                    item_type,
                    badge,
                    experience,
                    colors,
                    attrs,
                },
            file,
        } = form?;

        let name = name.trim();
        if name.is_empty() {
            return Err(MintItemError::EmptyName);
        }

        let descr = descr.trim();
        if descr.is_empty() {
            return Err(MintItemError::EmptyDescription);
        }

        let attrs: HashMap<String, AttrInfo> =
            serde_json::from_str(&attrs).map_err(|_| MintItemError::InvalidAttributes)?;

        for (attr, _) in &attrs {
            if !ATTRIBUTES.contains_key(attr.as_str()) {
                return Err(MintItemError::NoSuchAttribute(attr.clone()));
            }
        }

        let rarity: Rarity = rarity.parse()?;

        let item_type = match item_type.as_str() {
            "avatar" => {
                let filename =
                    Image::upload_image(file.ok_or(MintItemError::NoImageAttached)?.bytes)
                        .await?
                        .filename;
                ItemType::Avatar { filename }
            }
            "background" => {
                let colors = serde_json::from_str(&colors)?;
                ItemType::ProfileBackground { colors }
            }
            "reaction" => {
                let filename =
                    Image::upload_image(file.ok_or(MintItemError::NoImageAttached)?.bytes)
                        .await?
                        .filename;
                let xp_value: i32 = experience.parse()?;
                ItemType::Reaction { filename, xp_value }
            }
            "badge" => {
                let value = badge.trim().to_string();
                if value.is_empty() {
                    return Err(MintItemError::InvalidBadge);
                }
                ItemType::Badge {
                    value: badge.trim().to_string(),
                }
            }
            _ => return Err(MintItemError::InvalidItemType),
        };

        Ok(sqlx::query_as(
            r#"
            INSERT INTO items (name, description, available, rarity, item_type, attributes)
            VALUES ($1, $2, FALSE, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(name)
        .bind(descr)
        .bind(rarity)
        .bind(Jsonb(item_type))
        .bind(Jsonb(AttributeMap { attrs }))
        .fetch_one(&*conn)
        .await?)
    }
);

#[derive(Deserialize)]
pub struct GiftItemForm {
    receiver_id: i32,
    item_id:     i32,
    #[serde(default, deserialize_with = "crate::empty_string_as_none")]
    pattern:     Option<i32>,
}

#[derive(Debug, Error, Serialize, ErrorCode)]
pub enum GiftItemError {
    #[error("You are not authorized to gift items")]
    Unauthorized,
    #[error("Internal database error: {0}")]
    InternalDbError(
        #[from]
        #[serde(skip)]
        sqlx::Error,
    ),
}

post!(
    "/gift",
    #[json]
    async fn gift(
        conn: Extension<PgPool>,
        user: User,
        Form(GiftItemForm {
            receiver_id,
            item_id,
            pattern,
        }): Form<GiftItemForm>,
    ) -> Result<(), GiftItemError> {
        let pattern = pattern.unwrap_or_else(rand::random);

        if user.role < Role::Admin {
            return Err(GiftItemError::Unauthorized);
        }

        sqlx::query(
            r#"
            INSERT INTO drops (owner_id, item_id, pattern, consumed)
            VALUES ($1, $2, $3, FALSE)
            "#,
        )
        .bind(receiver_id)
        .bind(item_id)
        .bind(pattern)
        .execute(&*conn)
        .await?;

        Ok(())
    }
);
