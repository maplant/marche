use diesel::Queryable;
use serde::{Deserialize, Serialize};

/// Rarity of an item.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
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

/// An item that can be dropped.
#[derive(Queryable)]
pub struct Item {
    /// Id of the available item
    id: i32,
    /// Name of the item
    name: String,
    /// Description of the item
    description: String,
    /// Availability of the item (can the item be dropped?)
    available: bool,
    /// Rarity of the item
    rarity: Rarity,
    /// Type of the itemn
    item_type: ItemType,
}

/// A dropped item associated with a user.
#[derive(Queryable)]
pub struct DroppedItem {
    /// Id of the dropped item.
    id: i32,
    /// UserId of the owner.
    owner: i32,
    /// ItemId of the item.
    item: i32,
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
