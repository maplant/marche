use rocket_dyn_templates::*;

/// Displays an error message
#[rocket::get("/error/<message>")]
pub fn error(message: &str) -> Template {
    Template::render(
        "error",
        hashmap! {
            "message" => message.to_string(),
        },
    )
}
