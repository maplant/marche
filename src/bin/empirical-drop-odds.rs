use std::collections::HashMap;

use marche_server::items::Rarity;

fn main() {
    println!("pass = {}", libpasta::hash_password("test"));
    let mut items = HashMap::new();
    let mut rolls = 0;
    while rolls < 100000000 {
        let rarity = Rarity::roll();
        rolls += 1;
        *items.entry(rarity).or_insert(0_usize) += 1;
    }

    for (rarity, dropped) in items.into_iter() {
        println!(
            "{:?} rarity dropped {}% of the time",
            rarity,
            (dropped as f32 / rolls as f32) * 100.0
        );
    }
}
