use std::{cmp::PartialEq, collections::HashMap};

use axum::{
    extract::{Extension, Form, Path},
    Json,
};
use chrono::{Duration, Utc};
use derive_more::From;
use diesel::{
    pg::Pg,
    prelude::*,
    serialize::Output,
    sql_types::Jsonb,
    types::{FromSql, ToSql},
};
use diesel_derive_enum::DbEnum;
use lazy_static::lazy_static;
use maplit::hashmap;
use marche_proc_macros::json_result;
use rand::{
    prelude::{thread_rng, IteratorRandom},
    Rng, SeedableRng,
};
use rand_xorshift::XorShiftRng;
use serde::{Deserialize, Serialize};

use crate::{
    post,
    users::{ProfileStub, User, UserCache},
    PgPool,
};

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

/// The type of an item. Determines if the item has any associated actions or is
/// purely cosmetic, and further if the item is cosmetic how many can be
/// equipped
#[derive(Clone, Debug, Serialize, Deserialize, FromSqlRow, AsExpression)]
#[sql_type = "Jsonb"]
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

#[derive(Clone, Debug, FromSqlRow, AsExpression)]
#[sql_type = "Jsonb"]
pub struct AttributeMap {
    map: HashMap<String, AttrInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AttrInfo {
    rarity: u32,
    seed:   usize,
}

impl ToSql<Jsonb, Pg> for AttributeMap {
    fn to_sql<W: std::io::Write>(&self, out: &mut Output<'_, W, Pg>) -> diesel::serialize::Result {
        out.write_all(&[1])?;
        serde_json::to_writer(out, &self.map)
            .map(|_| diesel::serialize::IsNull::No)
            .map_err(Into::into)
    }
}

impl FromSql<Jsonb, Pg> for AttributeMap {
    fn from_sql(bytes: Option<&[u8]>) -> diesel::deserialize::Result<Self> {
        serde_json::from_slice(&bytes.unwrap_or(&[])[1..])
            .map(|map| Self { map })
            .map_err(|_| "Invalid Json".into())
    }
}

table! {
    use diesel::sql_types::*;
    use super::RarityMapping;

    items(id) {
        id -> Integer,
        name -> Text,
        description -> Text,
        available -> Bool,
        rarity -> RarityMapping,
        item_type -> Jsonb,
        attributes -> Jsonb,
    }
}

/// An item that can be dropped
#[derive(Queryable, Debug)]
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
    #[diesel(sql_type = "ItemType")]
    pub item_type:   ItemType,
    /// Attribute rarity
    #[diesel(sql_type = "AttributeMap")]
    pub attributes:  AttributeMap,
}

impl Item {
    pub fn fetch(conn: &PgConnection, item_id: i32) -> Self {
        use self::items::dsl::*;

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

    pub fn is_equipable(&self) -> bool {
        matches!(
            self.item_type,
            ItemType::Avatar { .. } | ItemType::ProfileBackground { .. } | ItemType::Badge { .. }
        )
    }
}

table! {
    drops(id) {
        id -> Integer,
        owner_id -> Integer,
        item_id -> Integer,
        pattern -> SmallInt,
        consumed -> Bool,
    }
}

/// A dropped item associated with a user
#[derive(Queryable, Debug)]
pub struct ItemDrop {
    /// Id of the dropped item
    pub id:       i32,
    /// UserId of the owner
    pub owner_id: i32,
    /// ItemId of the item
    pub item_id:  i32,
    /// Unique pattern Id for the item
    pub pattern:  i16,
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
    /// The maximum amount of time since the last drop until the drop is garanteed.
    static ref MAX_DROP_PERIOD: Duration = Duration::hours(23);
}

/// Chance of drop is equal to 1/DROP_CHANCE
pub const DROP_CHANCE: u32 = 2;

#[derive(Insertable)]
#[table_name = "drops"]
pub struct NewDrop {
    owner_id: i32,
    item_id:  i32,
    pattern:  i16,
    consumed: bool,
}

impl ItemDrop {
    pub fn fetch(conn: &PgConnection, drop_id: i32) -> Result<Self, diesel::result::Error> {
        use self::drops::dsl::*;

        drops.find(drop_id).first::<Self>(conn)
    }

    pub fn fetch_item(&self, conn: &PgConnection) -> Item {
        Item::fetch(conn, self.item_id)
    }

    /// Equips an item. Up to the caller to ensure current user owns the item.
    pub fn equip(&self, conn: &PgConnection) {
        use crate::users::users::dsl::*;

        let _ = conn.transaction(|| -> Result<(), diesel::result::Error> {
            use diesel::result::Error::RollbackTransaction;

            let user = User::fetch(conn, self.owner_id).map_err(|_| RollbackTransaction)?;
            let item_desc = Item::fetch(conn, self.item_id);
            let user = match item_desc.item_type {
                ItemType::Avatar { .. } => diesel::update(users.find(user.id))
                    .set(equip_slot_prof_pic.eq(Some(self.id)))
                    .get_result::<User>(conn)
                    .map_err(|_| RollbackTransaction)?,
                ItemType::ProfileBackground { .. } => diesel::update(users.find(user.id))
                    .set(equip_slot_background.eq(Some(self.id)))
                    .get_result::<User>(conn)
                    .map_err(|_| RollbackTransaction)?,
                ItemType::Badge { .. } => {
                    let mut badges = user.equip_slot_badges.clone();
                    if !badges.contains(&self.id) && badges.len() < crate::users::MAX_NUM_BADGES {
                        badges.push(self.id);
                    }
                    diesel::update(users.find(user.id))
                        .set(equip_slot_badges.eq(badges))
                        .get_result::<User>(conn)
                        .map_err(|_| RollbackTransaction)?
                }
                _ => return Err(RollbackTransaction),
            };
            if user.id == self.owner_id {
                Ok(())
            } else {
                Err(RollbackTransaction)
            }
        });
    }

    /// Unequips an item. Up to the caller to ensure current user owns the item.
    pub fn unequip(&self, conn: &PgConnection) {
        use crate::users::users::dsl::*;

        // No need for a transaction here
        let user = User::fetch(conn, self.owner_id).unwrap();
        let item_desc = Item::fetch(conn, self.item_id);
        match item_desc.item_type {
            ItemType::Avatar { .. } => {
                let _ = diesel::update(users.find(user.id))
                    .filter(equip_slot_prof_pic.eq(Some(self.id)))
                    .set(equip_slot_prof_pic.eq(Option::<i32>::None))
                    .get_result::<User>(conn);
            }
            ItemType::ProfileBackground { .. } => {
                let _ = diesel::update(users.find(user.id))
                    .filter(equip_slot_background.eq(Some(self.id)))
                    .set(equip_slot_background.eq(Option::<i32>::None))
                    .get_result::<User>(conn);
            }
            ItemType::Badge { .. } => {
                let badges = user
                    .equip_slot_badges
                    .iter()
                    .cloned()
                    .filter(|drop_id| *drop_id != self.id)
                    .collect::<Vec<_>>();
                let _ = diesel::update(users.find(user.id))
                    .set(equip_slot_badges.eq(badges))
                    .get_result::<User>(conn);
            }
            _ => (),
        }
    }

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
            id:          self.id,
            name:        item.name.clone(),
            rarity:      item.rarity.to_string(),
            thumbnail:   self.thumbnail_html(conn),
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

    /// Possibly selects an item, depending on the last drop.
    pub fn drop(conn: &PgConnection, user: &User) -> Option<Self> {
        // Determine if we have a drop
        conn.transaction(|| {
            let item: Option<Self> = (user.last_reward
                < (Utc::now() - *MAX_DROP_PERIOD).naive_utc()
                || user.last_reward < (Utc::now() - *MIN_DROP_PERIOD).naive_utc()
                    && rand::random::<u32>() <= (u32::MAX / DROP_CHANCE))
                .then(|| {
                    use self::items::dsl::*;

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
                                    item_id:  chosen.id,
                                    pattern:  rand::random(),
                                    consumed: false,
                                })
                                .get_result(conn)
                                .ok()
                        })
                })
                .flatten();

            if item.is_some() {
                // Update the last reward. This will fail if the user has seen a reward
                // since the start of this function.
                user.update_last_reward(conn)
                    .map_err(|_| diesel::result::Error::RollbackTransaction)
                    .map(move |_| item)
            } else {
                Ok(item)
            }
        })
        .ok()
        .flatten()
    }
}

#[derive(Serialize)]
pub enum EquipError {
    NoSuchItem,
    NotYourItem,
}

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
    pub id:          i32,
    pub name:        String,
    pub rarity:      String,
    pub thumbnail:   String,
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

table! {
    trade_requests(id) {
        id -> Integer,
        sender_id -> Integer,
        sender_items -> Array<Integer>,
        receiver_id -> Integer,
        receiver_items -> Array<Integer>,
        note -> Nullable<Text>,
    }
}

/// A trade between two users.
#[derive(Serialize, Queryable)]
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

#[derive(Serialize)]
pub enum FetchTradeRequestError {
    NoSuchTradeRequest,
    InternalDbError(#[serde(skip)] diesel::result::Error),
}

impl From<diesel::result::Error> for FetchTradeRequestError {
    fn from(err: diesel::result::Error) -> Self {
        match err {
            diesel::result::Error::NotFound => Self::NoSuchTradeRequest,
            x => Self::InternalDbError(x),
        }
    }
}

#[derive(Serialize, From)]
pub enum TradeResponseError {
    NoSuchTrade,
    NotYourTrade,
    InternalDbError(#[serde(skip)] diesel::result::Error),
}

impl From<FetchTradeRequestError> for TradeResponseError {
    fn from(err: FetchTradeRequestError) -> Self {
        match err {
            FetchTradeRequestError::NoSuchTradeRequest => Self::NoSuchTrade,
            FetchTradeRequestError::InternalDbError(err) => Self::InternalDbError(err),
        }
    }
}

impl TradeRequest {
    pub fn fetch(conn: &PgConnection, req_id: i32) -> Result<Self, FetchTradeRequestError> {
        use self::trade_requests::dsl::*;

        match trade_requests.find(req_id).first::<Self>(conn) {
            Ok(x) => Ok(x),
            Err(diesel::result::Error::NotFound) => Err(FetchTradeRequestError::NoSuchTradeRequest),
            Err(x) => Err(FetchTradeRequestError::InternalDbError(x)),
        }
    }

    pub fn accept(&self, conn: &PgConnection) -> Result<(), TradeResponseError> {
        use self::drops::dsl::*;

        conn.transaction(|| -> Result<(), diesel::result::Error> {
            for sender_item in &self.sender_items {
                diesel::update(drops.find(sender_item))
                    .set(owner_id.eq(self.receiver_id))
                    .filter(owner_id.eq(self.sender_id))
                    .get_result::<ItemDrop>(conn)
                    .map_err(|_| diesel::result::Error::RollbackTransaction)?
                    .unequip(conn);
            }
            for receiver_item in &self.receiver_items {
                diesel::update(drops.find(receiver_item))
                    .set(owner_id.eq(self.sender_id))
                    .filter(owner_id.eq(self.receiver_id))
                    .get_result::<ItemDrop>(conn)
                    .map_err(|_| diesel::result::Error::RollbackTransaction)?
                    .unequip(conn);
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
            let _ = self.decline(&conn);
            Ok(())
        })?;

        Ok(())
    }

    pub fn decline(&self, conn: &PgConnection) -> Result<(), TradeResponseError> {
        use self::trade_requests::dsl::*;

        diesel::delete(trade_requests.find(self.id)).execute(conn)?;

        Ok(())
    }
}

#[derive(Insertable)]
#[table_name = "trade_requests"]
pub struct NewTradeRequest {
    sender_id:      i32,
    sender_items:   Vec<i32>,
    receiver_id:    i32,
    receiver_items: Vec<i32>,
    note:           Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TradeRequestForm {
    receiver_id: String,
    note:        Option<String>,
    #[serde(flatten)]
    trade:       HashMap<String, String>,
}

#[derive(Serialize, From)]
pub enum SubmitOfferError {
    CannotTradeWithSelf,
    ItemNoLongerOwned,
    NoSuchUser,
    InvalidTrade,
    NoteTooLong,
    TradeIsEmpty,
    InternalDbError(#[serde(skip)] diesel::result::Error),
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

        let conn = pool.get().expect("Could not connect to db");

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

        Ok(diesel::insert_into(trade_requests::table)
            .values(&NewTradeRequest {
                sender_id: sender.id,
                sender_items,
                receiver_id,
                receiver_items,
                note,
            })
           .get_result::<TradeRequest>(&conn)?)
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
        let conn = pool.get().expect("Could not connect to db");
        let req = TradeRequest::fetch(&conn, trade_id)?;
        if req.receiver_id == user.id {
            req.accept(&conn)
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
        let conn = pool.get().expect("Could not connect to db");
        let req = TradeRequest::fetch(&conn, trade_id)?;
        if req.sender_id == user.id || req.receiver_id == user.id {
            req.decline(&conn)
        } else {
            Err(TradeResponseError::NotYourTrade)
        }
    }
}

#[derive(Serialize)]
pub struct IncomingOffer {
    pub id:             i32,
    pub sender:         ProfileStub,
    pub sender_items:   Vec<ItemThumbnail>,
    pub receiver_items: Vec<ItemThumbnail>,
    pub note:           Option<String>,
}

impl IncomingOffer {
    pub fn retrieve(
        conn: &PgConnection,
        user_cache: &mut UserCache,
        user: &User,
    ) -> Vec<IncomingOffer> {
        use self::trade_requests::dsl::*;

        return trade_requests
            .filter(receiver_id.eq(user.id))
            .load::<TradeRequest>(conn)
            .unwrap()
            .into_iter()
            .map(|trade| -> IncomingOffer {
                IncomingOffer {
                    id:             trade.id,
                    sender:         user_cache.get(trade.sender_id).clone(),
                    sender_items:   trade
                        .sender_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    receiver_items: trade
                        .receiver_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    note:           trade.note,
                }
            })
            .collect();
    }

    pub fn count(conn: &PgConnection, user: &User) -> i64 {
        use self::trade_requests::dsl::*;
        return trade_requests
            .filter(receiver_id.eq(user.id))
            .count()
            .get_result(conn)
            .unwrap_or(0);
    }
}

#[derive(Serialize)]
pub struct OutgoingOffer {
    pub id:             i32,
    pub sender_items:   Vec<ItemThumbnail>,
    pub receiver:       ProfileStub,
    pub receiver_items: Vec<ItemThumbnail>,
    pub note:           Option<String>,
}

impl OutgoingOffer {
    pub fn retrieve(
        conn: &PgConnection,
        user_cache: &mut UserCache,
        user: &User,
    ) -> Vec<OutgoingOffer> {
        use self::trade_requests::dsl::*;

        return trade_requests
            .filter(sender_id.eq(user.id))
            .load::<TradeRequest>(conn)
            .unwrap()
            .into_iter()
            .map(|trade| -> OutgoingOffer {
                OutgoingOffer {
                    id:             trade.id,
                    receiver:       user_cache.get(trade.receiver_id).clone(),
                    sender_items:   trade
                        .sender_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    receiver_items: trade
                        .receiver_items
                        .into_iter()
                        .map(|i| ItemDrop::fetch(&conn, i).unwrap().thumbnail(&conn))
                        .collect(),
                    note:           trade.note,
                }
            })
            .collect();
    }
}
