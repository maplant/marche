# ⚖️ Marche ⚖️

## What is Marche?

Marche is lightweight forum software in the vein of Reddit or 4chan, designed to provide an 
enjoyable and rewarding experience for users. 

You can check out the canonical deployment of marche at https://www.cest-le-marche.com
Everything on the main branch of this repository is automatically deployed to that URL.

## What makes Marche different?

The primary difference between Marche and other forum softwares is the inclusion of items. 
When a user posts a thread of a reply, there is a random chance that the user will be given 
an item. These items can include cosmetic items such a profile pictures or backgrounds and 
badges that the user can equip to show of their personality. 

The other type of item that can drop includes reactions. A user can use a reaction one time 
on any post other then one of their own to show appreciation or the opposite. Reactions add
or subtract experience points from the recipient. 

Experience points grant users special priviliges, the most prominent example as of right now
being the ability to attach photos to posts after a certain level. This change was made in 
order to reduce the chance that users post blatantly illegal photos.

## Ethos

Marche is designed to be fun. It is not designed to revolutionize communication or society or 
how people interact with each other online. 

## Technical details

Marche is written in Rust and uses the following tech stack:

 * Tokio (async runtime)
 * Axum (web framework)
 * Askama (templating)
 * Jquery 
 


